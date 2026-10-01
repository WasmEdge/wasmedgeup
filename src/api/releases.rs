use std::{any::Any, borrow::Cow, io};

use gix::{
    bstr::BStr,
    protocol::transport::{
        client::{
            blocking_io::{
                connect,
                http::{self, GetResponse, Http, PostBodyDataKind, PostResponse},
                ExtendedBufRead, HandleProgress, ReadlineBufRead, RequestWriter,
                SetServiceResponse, Transport,
            },
            Account, Error as TransportError, MessageKind, TransportWithoutIO, WriteMode,
        },
        packetline::PacketLineRef,
        Protocol, Service,
    },
    remote::Direction,
};
use semver::Version;

use crate::prelude::*;

const MAX_GIT_RESPONSE_BYTES: u64 = 1024 * 1024;

struct BoundedReader<R> {
    inner: R,
    limit: u64,
    remaining: u64,
}

impl<R> BoundedReader<R> {
    fn new(inner: R, limit: u64) -> Self {
        Self {
            inner,
            limit,
            remaining: limit,
        }
    }

    fn limit_error(limit: u64) -> io::Error {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("remote Git response exceeds {limit} bytes"),
        )
    }

    fn charge(&mut self, amount: usize) -> io::Result<()> {
        let amount = u64::try_from(amount).unwrap_or(u64::MAX);
        if amount > self.remaining {
            self.remaining = 0;
            return Err(Self::limit_error(self.limit));
        }
        self.remaining -= amount;
        Ok(())
    }
}

impl<R: io::BufRead> io::Read for BoundedReader<R> {
    fn read(&mut self, output: &mut [u8]) -> io::Result<usize> {
        if output.is_empty() {
            return Ok(0);
        }

        let available = io::BufRead::fill_buf(self)?;
        let amount = available.len().min(output.len());
        output[..amount].copy_from_slice(&available[..amount]);
        io::BufRead::consume(self, amount);
        Ok(amount)
    }
}

impl<R: io::BufRead> io::BufRead for BoundedReader<R> {
    fn fill_buf(&mut self) -> io::Result<&[u8]> {
        let limit = self.limit;
        let available = self.inner.fill_buf()?;
        if self.remaining == 0 {
            if available.is_empty() {
                return Ok(available);
            }
            return Err(Self::limit_error(limit));
        }

        let remaining = usize::try_from(self.remaining).unwrap_or(usize::MAX);
        Ok(&available[..available.len().min(remaining)])
    }

    fn consume(&mut self, amount: usize) {
        let amount = amount.min(usize::try_from(self.remaining).unwrap_or(usize::MAX));
        self.remaining -= amount as u64;
        self.inner.consume(amount);
    }
}

impl<R: ReadlineBufRead> ReadlineBufRead for BoundedReader<R> {
    fn readline(&mut self) -> Option<io::Result<gix::ExnMessageResult<PacketLineRef<'_>>>> {
        let limit = self.limit;
        let remaining = &mut self.remaining;
        let result = self.inner.readline()?;
        match result {
            Ok(Ok(line)) => {
                let amount = line
                    .as_slice()
                    .map_or(4, |data| data.len().saturating_add(4));
                let amount = u64::try_from(amount).unwrap_or(u64::MAX);
                if amount > *remaining {
                    *remaining = 0;
                    Some(Err(Self::limit_error(limit)))
                } else {
                    *remaining -= amount;
                    Some(Ok(Ok(line)))
                }
            }
            result => Some(result),
        }
    }

    fn readline_str(&mut self, line: &mut String) -> io::Result<usize> {
        let amount = self.inner.readline_str(line)?;
        if amount != 0 {
            self.charge(amount.saturating_add(4))?;
        }
        Ok(amount)
    }
}

impl<'a, R: ExtendedBufRead<'a>> ExtendedBufRead<'a> for BoundedReader<R> {
    fn set_progress_handler(&mut self, handle_progress: Option<HandleProgress<'a>>) {
        self.inner.set_progress_handler(handle_progress);
    }

    fn peek_data_line(&mut self) -> Option<io::Result<std::result::Result<&[u8], TransportError>>> {
        self.inner.peek_data_line()
    }

    fn reset(&mut self, version: Protocol) {
        self.inner.reset(version);
    }

    fn stopped_at(&self) -> Option<MessageKind> {
        self.inner.stopped_at()
    }
}

struct BoundedHttp<H> {
    inner: H,
    response_limit: u64,
}

impl<H> BoundedHttp<H> {
    fn new(inner: H, response_limit: u64) -> Self {
        Self {
            inner,
            response_limit,
        }
    }
}

impl<H: Http> Http for BoundedHttp<H> {
    type Headers = H::Headers;
    type ResponseBody = BoundedReader<H::ResponseBody>;
    type PostBody = H::PostBody;

    fn get(
        &mut self,
        url: &str,
        base_url: &str,
        headers: impl IntoIterator<Item = impl AsRef<str>>,
    ) -> gix::ExnMessageResult<GetResponse<Self::Headers, Self::ResponseBody>> {
        self.inner
            .get(url, base_url, headers)
            .map(|response| GetResponse {
                headers: response.headers,
                body: BoundedReader::new(response.body, self.response_limit),
            })
    }

    fn post(
        &mut self,
        url: &str,
        base_url: &str,
        headers: impl IntoIterator<Item = impl AsRef<str>>,
        body: PostBodyDataKind,
    ) -> gix::ExnMessageResult<PostResponse<Self::Headers, Self::ResponseBody, Self::PostBody>>
    {
        self.inner
            .post(url, base_url, headers, body)
            .map(|response| PostResponse {
                post_body: response.post_body,
                headers: response.headers,
                body: BoundedReader::new(response.body, self.response_limit),
            })
    }

    fn configure(&mut self, config: &dyn Any) -> gix::ExnResult {
        self.inner.configure(config)
    }

    fn redirected_base_url(&self) -> Option<String> {
        self.inner.redirected_base_url()
    }
}

/// Applies a byte budget to packet-line response readers exposed by any gix
/// transport. HTTP additionally has its raw response body limited by
/// `BoundedHttp`, while this wrapper extends the same protection to SSH,
/// `git://`, and local/file transports.
struct BoundedTransport<T> {
    inner: T,
    response_limit: u64,
}

impl<T> BoundedTransport<T> {
    fn new(inner: T, response_limit: u64) -> Self {
        Self {
            inner,
            response_limit,
        }
    }
}

impl<T: TransportWithoutIO> TransportWithoutIO for BoundedTransport<T> {
    fn set_identity(&mut self, identity: Account) -> std::result::Result<(), TransportError> {
        self.inner.set_identity(identity)
    }

    fn to_url(&self) -> Cow<'_, BStr> {
        self.inner.to_url()
    }

    fn supported_protocol_versions(&self) -> &[Protocol] {
        self.inner.supported_protocol_versions()
    }

    fn connection_persists_across_multiple_requests(&self) -> bool {
        self.inner.connection_persists_across_multiple_requests()
    }

    fn configure(&mut self, config: &dyn Any) -> gix::ExnResult {
        self.inner.configure(config)
    }
}

impl<T: Transport> Transport for BoundedTransport<T> {
    fn handshake<'a>(
        &mut self,
        service: Service,
        extra_parameters: &'a [(&'a str, Option<&'a str>)],
    ) -> std::result::Result<SetServiceResponse<'_>, TransportError> {
        let response = self.inner.handshake(service, extra_parameters)?;
        Ok(SetServiceResponse {
            actual_protocol: response.actual_protocol,
            capabilities: response.capabilities,
            refs: response.refs.map(|refs| {
                Box::new(BoundedReader::new(refs, self.response_limit)) as Box<dyn ReadlineBufRead>
            }),
        })
    }

    fn request(
        &mut self,
        write_mode: WriteMode,
        on_into_read: MessageKind,
        trace: bool,
    ) -> std::result::Result<RequestWriter<'_>, TransportError> {
        let request = self.inner.request(write_mode, on_into_read, trace)?;
        let (writer, reader) = request.into_parts();
        let reader = Box::new(BoundedReader::new(reader, self.response_limit));
        Ok(RequestWriter::new_from_bufread(
            writer,
            reader,
            write_mode,
            on_into_read,
            trace,
        ))
    }
}

#[derive(Debug, Clone, Copy)]
pub enum ReleasesFilter {
    All,
    Stable,
}

impl ReleasesFilter {
    pub fn matches(self, semver: &semver::Version) -> bool {
        match self {
            Self::All => true,
            Self::Stable => semver.pre.is_empty(),
        }
    }
}

fn protocol_for_bounded_handshake(scheme: &gix::url::Scheme, preferred: Protocol) -> Protocol {
    if matches!(scheme, gix::url::Scheme::Http | gix::url::Scheme::Https) {
        preferred
    } else {
        // gix collects every protocol-v2 capability packet inside
        // `Transport::handshake`, before `BoundedTransport` can wrap the
        // returned reader. Protocol v1 reads only its intrinsically bounded
        // first packet during the handshake and exposes the ref stream for our
        // wrapper to limit. Servers may still downgrade this request to v0,
        // which has the same bounded handshake shape.
        Protocol::V1
    }
}

/// List every release tag advertised by the remote, sorted newest-first.
///
/// Performs a `git ls-remote`-style ref discovery against `url` using the
/// pure-Rust `gix` stack (`rustls` for TLS). No objects are downloaded —
/// we only consume the ref advertisement returned during the protocol
/// handshake. Advertised ref packet streams are limited to
/// `MAX_GIT_RESPONSE_BYTES` for every supported transport; smart-HTTP bodies
/// are also limited before gix parses them. Other transports request protocol
/// v1 so gix exposes refs after one bounded packet instead of eagerly
/// collecting a protocol-v2 capability stream inside the handshake.
pub fn get_all(url: &str, filter: ReleasesFilter) -> Result<Vec<Version>> {
    get_all_with_response_limit(url, filter, MAX_GIT_RESPONSE_BYTES)
}

fn get_all_with_response_limit(
    url: &str,
    filter: ReleasesFilter,
    response_limit: u64,
) -> Result<Vec<Version>> {
    // gix's high-level Connection API requires a repository handle, so we
    // initialise an ephemeral bare repo skeleton in a tempdir. `init_bare`
    // does write the empty repo metadata (HEAD, config, refs/, objects/)
    // there; what we never write is any fetched object — `ref_map` only
    // consumes the protocol's ref advertisement.
    let temp = tempfile::tempdir().map_err(|e| Error::Io {
        action: "create temp dir for git ls-remote".to_string(),
        path: std::env::temp_dir().display().to_string(),
        source: e,
    })?;

    // Use `Options::isolated()` so the repo handle does not load the user's
    // `~/.gitconfig` or the system `/etc/gitconfig`. Without this, gix would
    // honour `url.<base>.insteadOf` rewrites from those files and silently
    // redirect our `https://…` URL to SSH or to a mirror — a regression
    // versus `git2::Remote::create_detached`, which had no repo and so no
    // config to consult.
    let repo = gix::ThreadSafeRepository::init_opts(
        temp.path(),
        gix::create::Kind::Bare,
        gix::create::Options::default(),
        gix::open::Options::isolated(),
    )
    .map_err(|e| Error::Git {
        source: Box::new(e),
        resource: "init",
    })?
    .to_thread_local();

    // `Tags::All` makes gix include `+refs/tags/*:refs/tags/*` in the
    // effective refspecs, so the server advertises every tag — matching
    // the unconditional behaviour of git2::Remote::list. Without this we
    // would inherit the default `Tags::Included` mode, which omits tags
    // not reachable from the remote's selected branch tips and would
    // silently drop release tags that live on disconnected history.
    let remote = repo
        .remote_at(url)
        .map_err(|e| Error::Git {
            source: Box::new(e),
            resource: "remote",
        })?
        .with_fetch_tags(gix::remote::fetch::Tags::All);

    let (url, protocol) = remote
        .sanitized_url_and_version(Direction::Fetch)
        .map_err(|e| Error::Git {
            source: Box::new(e),
            resource: "remote/connect",
        })?;
    let protocol = protocol_for_bounded_handshake(&url.scheme, protocol);

    if matches!(url.scheme, gix::url::Scheme::Http | gix::url::Scheme::Https) {
        let git_http = BoundedHttp::new(http::reqwest::Remote::default(), response_limit);
        let transport = http::connect_http(git_http, url, protocol, false);
        return list_versions_with_transport(
            &remote,
            BoundedTransport::new(transport, response_limit),
            filter,
        );
    }

    // Use gix's general connector for every other supported scheme so SSH
    // options, local path normalization, and protocol selection retain their
    // existing behaviour. The wrapper limits the advertised packet stream
    // before `ref_map` can materialize it.
    let ssh = (url.scheme == gix::url::Scheme::Ssh)
        .then(|| repo.ssh_connect_options())
        .transpose()
        .map_err(|e| Error::Git {
            source: Box::new(e),
            resource: "remote/connect",
        })?
        .unwrap_or_default();
    let transport = connect::connect(
        url,
        connect::Options {
            version: protocol,
            ssh,
            trace: false,
        },
    )
    .map_err(|e| Error::Git {
        source: Box::new(io::Error::other(e.to_string())),
        resource: "remote/connect",
    })?;
    list_versions_with_transport(
        &remote,
        BoundedTransport::new(transport, response_limit),
        filter,
    )
}

fn list_versions_with_transport<T: Transport>(
    remote: &gix::Remote<'_>,
    transport: T,
    filter: ReleasesFilter,
) -> Result<Vec<Version>> {
    let (ref_map, _handshake) = remote
        .to_connection_with_transport(transport)
        .ref_map(
            gix::progress::Discard,
            gix::remote::ref_map::Options::default(),
        )
        .map_err(|e| Error::Git {
            source: Box::new(e),
            resource: "remote/ref_map",
        })?;

    let mut heads: Vec<Version> = ref_map
        .remote_refs
        .iter()
        .filter_map(remote_ref_to_version)
        .filter(|version| filter.matches(version))
        .collect();
    heads.sort_unstable_by(|a, b| b.cmp(a));

    Ok(heads)
}

fn remote_ref_to_version(r: &gix::protocol::handshake::Ref) -> Option<Version> {
    let (name_bstr, _target, _peeled) = r.unpack();
    let name = std::str::from_utf8(name_bstr.as_ref()).ok()?;
    parse_tag_ref(name)
}

/// Parse a fully-qualified ref name into a semver `Version` if it represents
/// a release tag we recognise. Returns `None` for non-tag refs, peeled tags
/// (`^{}` suffix), and tag names that don't parse as semver.
fn parse_tag_ref(ref_name: &str) -> Option<Version> {
    let name = ref_name.strip_prefix("refs/tags/")?;
    if name.ends_with("^{}") {
        return None;
    }
    Version::parse(name).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Cursor, Read};

    #[derive(Clone)]
    struct StaticHttp {
        body: Vec<u8>,
    }

    impl Http for StaticHttp {
        type Headers = Cursor<Vec<u8>>;
        type ResponseBody = Cursor<Vec<u8>>;
        type PostBody = Cursor<Vec<u8>>;

        fn get(
            &mut self,
            _url: &str,
            _base_url: &str,
            _headers: impl IntoIterator<Item = impl AsRef<str>>,
        ) -> gix::ExnMessageResult<GetResponse<Self::Headers, Self::ResponseBody>> {
            Ok(GetResponse {
                headers: Cursor::new(Vec::new()),
                body: Cursor::new(self.body.clone()),
            })
        }

        fn post(
            &mut self,
            _url: &str,
            _base_url: &str,
            _headers: impl IntoIterator<Item = impl AsRef<str>>,
            _body: PostBodyDataKind,
        ) -> gix::ExnMessageResult<PostResponse<Self::Headers, Self::ResponseBody, Self::PostBody>>
        {
            Ok(PostResponse {
                post_body: Cursor::new(Vec::new()),
                headers: Cursor::new(Vec::new()),
                body: Cursor::new(self.body.clone()),
            })
        }

        fn configure(&mut self, _config: &dyn Any) -> gix::ExnResult {
            Ok(())
        }
    }

    #[test]
    fn bounded_reader_accepts_response_at_limit() {
        let mut reader = BoundedReader::new(Cursor::new(b"1234"), 4);
        let mut body = Vec::new();

        reader.read_to_end(&mut body).expect("read within limit");

        assert_eq!(body, b"1234");
    }

    #[test]
    fn bounded_reader_rejects_response_over_limit() {
        let mut reader = BoundedReader::new(Cursor::new(b"12345"), 4);
        let mut body = Vec::new();

        let error = reader
            .read_to_end(&mut body)
            .expect_err("a response larger than the limit must be rejected");

        assert_eq!(error.kind(), std::io::ErrorKind::InvalidData);
        assert!(error.to_string().contains("exceeds 4 bytes"));
    }

    #[test]
    fn bounded_reader_rejects_packet_stream_over_limit() {
        let encoded = Cursor::new(b"0008test0008more0000");
        let mut packets =
            gix::protocol::transport::packetline::blocking_io::StreamingPeekableIter::new(
                encoded,
                &[PacketLineRef::Flush],
                false,
            );
        let mut reader = BoundedReader::new(packets.as_read(), 12);

        let first = reader
            .readline()
            .expect("first packet")
            .expect("read first packet")
            .expect("decode first packet");
        assert_eq!(first.as_slice(), Some(b"test".as_slice()));

        let error = reader
            .readline()
            .expect("second packet")
            .expect_err("cumulative packet bytes must be limited");
        assert_eq!(error.kind(), std::io::ErrorKind::InvalidData);
        assert!(error.to_string().contains("exceeds 12 bytes"));
    }

    #[test]
    fn bounded_http_limits_get_and_post_response_bodies() {
        let backend = StaticHttp {
            body: b"12345".to_vec(),
        };
        let mut http = BoundedHttp::new(backend, 4);

        let mut get_body = http
            .get(
                "http://example.test/info",
                "http://example.test",
                ["accept: test"],
            )
            .expect("GET response")
            .body;
        assert!(get_body.read_to_end(&mut Vec::new()).is_err());

        let mut post_body = http
            .post(
                "http://example.test/upload",
                "http://example.test",
                ["accept: test"],
                PostBodyDataKind::BoundedAndFitsIntoMemory,
            )
            .expect("POST response")
            .body;
        assert!(post_body.read_to_end(&mut Vec::new()).is_err());
    }

    #[test]
    fn get_all_supports_local_file_remotes() {
        let remote_dir = tempfile::tempdir().expect("create remote directory");
        let remote = gix::init_bare(remote_dir.path()).expect("initialize bare remote");
        let target = remote
            .write_blob(b"release tag target")
            .expect("write tag target")
            .detach();
        remote
            .reference(
                "refs/tags/1.2.3",
                target,
                gix::refs::transaction::PreviousValue::Any,
                "create release tag",
            )
            .expect("create release tag");

        let releases = get_all(
            remote_dir.path().to_str().expect("UTF-8 temp path"),
            ReleasesFilter::All,
        )
        .expect("list local release tags");

        assert_eq!(releases, vec![Version::new(1, 2, 3)]);
    }

    #[test]
    fn get_all_limits_local_file_remote_ref_advertisements() {
        let remote_dir = tempfile::tempdir().expect("create remote directory");
        let remote = gix::init_bare(remote_dir.path()).expect("initialize bare remote");
        let target = remote
            .write_blob(b"release tag target")
            .expect("write tag target")
            .detach();
        remote
            .reference(
                "refs/tags/1.2.3",
                target,
                gix::refs::transaction::PreviousValue::Any,
                "create release tag",
            )
            .expect("create release tag");

        let error = get_all_with_response_limit(
            remote_dir.path().to_str().expect("UTF-8 temp path"),
            ReleasesFilter::All,
            1,
        )
        .expect_err("local ref advertisements must respect the response limit");

        let message = format!("{error:?}");
        assert!(
            message.contains("exceeds 1 bytes"),
            "unexpected error: {message}"
        );
    }

    #[test]
    fn non_http_transports_avoid_unbounded_v2_handshakes() {
        for scheme in [
            gix::url::Scheme::File,
            gix::url::Scheme::Git,
            gix::url::Scheme::Ssh,
        ] {
            assert_eq!(
                protocol_for_bounded_handshake(&scheme, Protocol::V2),
                Protocol::V1
            );
        }

        assert_eq!(
            protocol_for_bounded_handshake(&gix::url::Scheme::Https, Protocol::V2),
            Protocol::V2
        );
    }

    #[test]
    fn parses_simple_release_tag() {
        let v = parse_tag_ref("refs/tags/0.14.1").unwrap();
        assert_eq!(v, Version::new(0, 14, 1));
    }

    #[test]
    fn parses_prerelease_tag() {
        let v = parse_tag_ref("refs/tags/0.15.0-alpha.1").unwrap();
        assert_eq!(v.major, 0);
        assert_eq!(v.minor, 15);
        assert_eq!(v.patch, 0);
        assert_eq!(v.pre.as_str(), "alpha.1");
    }

    #[test]
    fn rejects_peeled_tag() {
        assert!(parse_tag_ref("refs/tags/0.14.1^{}").is_none());
    }

    #[test]
    fn rejects_non_tag_ref() {
        assert!(parse_tag_ref("refs/heads/master").is_none());
        assert!(parse_tag_ref("HEAD").is_none());
    }

    #[test]
    fn rejects_non_semver_tag() {
        assert!(parse_tag_ref("refs/tags/not-a-version").is_none());
        assert!(parse_tag_ref("refs/tags/v0.14.1").is_none());
    }

    #[test]
    fn rejects_empty() {
        assert!(parse_tag_ref("").is_none());
        assert!(parse_tag_ref("refs/tags/").is_none());
    }
}

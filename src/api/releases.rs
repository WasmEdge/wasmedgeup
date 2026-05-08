use gix::remote::Direction;
use semver::Version;

use crate::prelude::*;

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

/// List every release tag advertised by the remote, sorted newest-first.
///
/// Performs a `git ls-remote`-style ref discovery against `url` using the
/// pure-Rust `gix` stack (`rustls` for TLS). No objects are downloaded —
/// we only consume the ref advertisement returned during the protocol
/// handshake.
pub fn get_all(url: &str, filter: ReleasesFilter) -> Result<Vec<Version>> {
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

    let connection = remote.connect(Direction::Fetch).map_err(|e| Error::Git {
        source: Box::new(e),
        resource: "remote/connect",
    })?;

    let (ref_map, _handshake) = connection
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

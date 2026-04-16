use std::{
    fmt::Write,
    io::{Read, Seek},
    path::Path,
    sync::OnceLock,
};

use crate::{
    constants::{
        CHECKSUM_FILE_NAME, DEFAULT_CONNECT_TIMEOUT_SECS, DEFAULT_REQUEST_TIMEOUT_SECS,
        DOWNLOAD_BUFFER_SIZE, WASMEDGE_GH_RELEASE_TAG_API, WASMEDGE_GIT_URL,
        WASMEDGE_RELEASE_BASE_URL,
    },
    http::HttpClientConfig,
    prelude::*,
    target::{TargetArch, TargetOS},
};
pub mod releases;
use indicatif::{ProgressBar, ProgressState, ProgressStyle};
pub use releases::ReleasesFilter;

use reqwest::{Client, Response};
use semver::{Comparator, Prerelease, Version, VersionReq};
use sha2::{Digest, Sha256};
use snafu::ResultExt;
use tempfile::NamedTempFile;
use tokio::{
    fs::{File, OpenOptions},
    io::AsyncWriteExt,
};
use url::Url;

#[derive(Debug, Clone)]
pub struct WasmEdgeApiClient {
    /// Connection timeout in seconds
    pub connect_timeout: u64,
    /// Request timeout in seconds
    pub request_timeout: u64,
}

impl WasmEdgeApiClient {
    fn http_client(&self) -> Result<Client> {
        HttpClientConfig::new()
            .with_connect_timeout(self.connect_timeout)
            .with_request_timeout(self.request_timeout)
            .build()
    }

    pub fn releases(&self, filter: ReleasesFilter, num_releases: usize) -> Result<Vec<Version>> {
        let releases = releases::get_all(WASMEDGE_GIT_URL, filter)?;
        Ok(releases.into_iter().take(num_releases).collect())
    }

    pub fn latest_release(&self) -> Result<Version> {
        let releases = releases::get_all(WASMEDGE_GIT_URL, ReleasesFilter::Stable)?;
        releases.into_iter().next().ok_or(Error::NoReleasesFound)
    }

    pub fn resolve_version(&self, version: &str) -> Result<Version> {
        if version == "latest" {
            self.latest_release()
        } else {
            Version::parse(version).context(SemVerSnafu {})
        }
    }

    pub async fn download_asset(
        &self,
        asset: &Asset,
        tmpdir: impl AsRef<Path>,
        no_progress: bool,
    ) -> Result<NamedTempFile> {
        let url = asset.url()?;
        tracing::debug!(%url, "Starting download for asset");

        let client = self.http_client()?;
        let response = client.get(url).send().await.context(RequestSnafu {
            resource: "asset download",
        })?;

        let named = NamedTempFile::new_in(tmpdir)?;
        let mut async_file = OpenOptions::new().write(true).open(named.path()).await?;

        stream_response_to_file(no_progress, response, &mut async_file).await?;
        drop(async_file);

        Ok(named)
    }

    pub async fn get_release_checksum(&self, version: &Version, asset: &Asset) -> Result<String> {
        let mut url = Url::parse(WASMEDGE_RELEASE_BASE_URL)
            .expect("WASMEDGE_RELEASE_BASE_URL must be a valid URL");

        url.path_segments_mut()
            .expect("base is valid URL")
            .extend(&[&version.to_string(), CHECKSUM_FILE_NAME]);

        tracing::debug!(%url, CHECKSUM_FILE_NAME, "Trying checksum file");

        let client = self.http_client()?;
        let response = client.get(url).send().await.context(RequestSnafu {
            resource: "checksums",
        })?;

        if !response.status().is_success() {
            tracing::debug!(
                status = %response.status(),
                file = CHECKSUM_FILE_NAME,
                "Checksum file not found"
            );
            return Err(Error::ChecksumNotFound {
                version: version.to_string(),
                asset: asset.archive_name.clone(),
            });
        }

        let content = response.text().await.context(RequestSnafu {
            resource: "checksums",
        })?;

        tracing::debug!(
            lines = content.lines().count(),
            file = CHECKSUM_FILE_NAME,
            "Got checksum file content"
        );

        for (i, line) in content.lines().enumerate() {
            tracing::debug!(line_num = i, line = line, "Processing checksum line");

            let parts: Vec<&str> = line.split_whitespace().collect();
            if parts.len() == 2 {
                tracing::debug!(checksum = parts[0], file = parts[1], "Found checksum entry");

                if parts[1] == asset.archive_name {
                    tracing::debug!(checksum = parts[0], "Found matching checksum");
                    return Ok(parts[0].to_string());
                }
            }
        }

        tracing::error!(
            version = %version,
            asset = %asset.archive_name,
            "No checksum found in any file"
        );

        Err(Error::ChecksumNotFound {
            version: version.to_string(),
            asset: asset.archive_name.clone(),
        })
    }

    pub async fn verify_file_checksum(file: &mut std::fs::File, expected: &str) -> Result<()> {
        let mut hasher = Sha256::new();
        let mut buffer = vec![0; DOWNLOAD_BUFFER_SIZE];

        loop {
            let count = file.read(&mut buffer)?;
            if count == 0 {
                break;
            }
            hasher.update(&buffer[..count]);
        }

        let actual = hex::encode(hasher.finalize());
        if actual != expected {
            return Err(Error::ChecksumMismatch {
                expected: expected.to_string(),
                actual,
            });
        }

        file.rewind()?;
        Ok(())
    }

    /// Download `url` to the file at `to`, streaming chunks and optionally
    /// showing a progress bar. The target file is created or truncated; its
    /// parent directory must already exist.
    ///
    /// `resource` is the label used in any [`Error::Request`] surfaced from
    /// this call — pass something descriptive of *what* the caller is
    /// downloading (e.g. `"plugin download"`) so user-facing errors stay
    /// specific instead of collapsing every download into a generic label.
    pub async fn download_to_path(
        &self,
        url: Url,
        to: &Path,
        no_progress: bool,
        resource: &'static str,
    ) -> Result<()> {
        tracing::debug!(%url, target = %to.display(), %resource, "Starting download to path");

        let client = self.http_client()?;
        let response = client
            .get(url)
            .send()
            .await
            .context(RequestSnafu { resource })?
            .error_for_status()
            .context(RequestSnafu { resource })?;

        let mut async_file = OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(to)
            .await?;
        stream_response_to_file(no_progress, response, &mut async_file).await?;
        Ok(())
    }

    /// Fetch plugin asset metadata from the GitHub Releases API for `tag`.
    ///
    /// A 404 (tag doesn't exist or has no published assets) yields an empty
    /// Vec rather than an error — callers treat "no assets" and "tag not
    /// found" the same way. Other non-2xx statuses (403 rate-limit, 5xx
    /// outage, etc.) and JSON parse failures are surfaced as typed errors.
    pub async fn github_release_assets(&self, tag: &str) -> Result<Vec<PluginAssetInfo>> {
        let url = format!("{WASMEDGE_GH_RELEASE_TAG_API}/{tag}");
        let client = self.http_client()?;
        let resp = client.get(&url).send().await.context(RequestSnafu {
            resource: "plugin release metadata",
        })?;
        if resp.status() == reqwest::StatusCode::NOT_FOUND {
            tracing::debug!(tag, "release metadata 404 — tag has no published assets");
            return Ok(Vec::new());
        }
        let resp = resp.error_for_status().context(RequestSnafu {
            resource: "plugin release metadata",
        })?;
        let text = resp.text().await.context(RequestSnafu {
            resource: "plugin release metadata body",
        })?;
        let v: serde_json::Value = serde_json::from_str(&text).context(JsonSnafu {
            resource: "plugin release metadata",
        })?;
        let mut out = Vec::new();
        if let Some(arr) = v.get("assets").and_then(|a| a.as_array()) {
            for a in arr {
                let name = a.get("name").and_then(|s| s.as_str()).unwrap_or("");
                if !name.starts_with(PLUGIN_ASSET_PREFIX) {
                    continue;
                }
                if let Some((plugin, version, platform)) = parse_plugin_asset_name(name, tag) {
                    out.push(PluginAssetInfo {
                        plugin,
                        version,
                        platform,
                    });
                }
            }
        }
        Ok(out)
    }

    /// Returns `true` if `url` responds with a successful status to HEAD
    /// (or GET, as a fallback for servers that disallow HEAD).
    pub async fn head_ok(&self, url: Url) -> bool {
        let Ok(client) = self.http_client() else {
            return false;
        };
        if let Ok(resp) = client.head(url.clone()).send().await {
            if resp.status().is_success() {
                return true;
            }
        }
        if let Ok(resp) = client.get(url).send().await {
            return resp.status().is_success();
        }
        false
    }
}

impl WasmEdgeApiClient {
    pub fn new() -> Self {
        Self {
            connect_timeout: DEFAULT_CONNECT_TIMEOUT_SECS,
            request_timeout: DEFAULT_REQUEST_TIMEOUT_SECS,
        }
    }

    pub fn with_connect_timeout(mut self, timeout: u64) -> Self {
        self.connect_timeout = timeout;
        self
    }

    pub fn with_request_timeout(mut self, timeout: u64) -> Self {
        self.request_timeout = timeout;
        self
    }
}

impl Default for WasmEdgeApiClient {
    fn default() -> Self {
        Self::new()
    }
}

#[tracing::instrument(level = tracing::Level::DEBUG, skip(response, target_file), fields(size = response.content_length()))]
async fn stream_response_to_file(
    no_progress: bool,
    mut response: Response,
    target_file: &mut File,
) -> Result<()> {
    let content_length = response.content_length().unwrap_or(0);

    let pb = if !no_progress && content_length > 0 {
        Some(download_progress_bar(
            response.content_length().unwrap_or_default(),
        ))
    } else {
        None
    };

    while let Some(mut chunk) = response
        .chunk()
        .await
        .context(RequestSnafu { resource: "chunk" })?
    {
        if let Some(ref pb) = pb {
            pb.inc(chunk.len() as u64)
        }
        target_file.write_buf(&mut chunk).await?;
    }

    target_file.flush().await?;

    if let Some(ref pb) = pb {
        pb.finish_and_clear();
    }

    Ok(())
}

#[derive(Debug, Clone)]
pub struct Asset {
    pub version: Version,
    pub archive_name: String,
    pub install_name: String,
}

impl Asset {
    pub fn new(version: &Version, os: &TargetOS, arch: &TargetArch) -> Self {
        Self {
            version: version.clone(),
            archive_name: Self::format_archive_name(version, os, arch),
            install_name: Self::format_install_name(version, os),
        }
    }

    pub fn url(&self) -> Result<Url> {
        let mut url = Url::parse(WASMEDGE_RELEASE_BASE_URL)
            .expect("WASMEDGE_RELEASE_BASE_URL must be a valid URL");

        url.path_segments_mut()
            .expect("base is valid URL")
            .extend(&[&self.version.to_string(), &self.archive_name]);

        Ok(url)
    }

    fn format_archive_name(version: &Version, os: &TargetOS, arch: &TargetArch) -> String {
        use TargetArch as Arch;
        use TargetOS as OS;

        match (os, arch) {
            (OS::Ubuntu, Arch::X86_64) => {
                format!("WasmEdge-{version}-ubuntu20.04_x86_64.tar.gz")
            }
            (OS::Ubuntu, Arch::Aarch64) if is_arm_ubuntu_supported(version) => {
                format!("WasmEdge-{version}-ubuntu20.04_aarch64.tar.gz")
            }
            (OS::Linux | OS::Ubuntu, arch) => {
                let manylinux_version = if is_manylinux2014_supported(version) {
                    "manylinux2014"
                } else {
                    "manylinux_2_28"
                };
                let arch = match arch {
                    Arch::X86_64 => "x86_64",
                    Arch::Aarch64 => "aarch64",
                };
                format!("WasmEdge-{version}-{manylinux_version}_{arch}.tar.gz")
            }
            (OS::Darwin, Arch::X86_64) => {
                format!("WasmEdge-{version}-darwin_x86_64.tar.gz")
            }
            (OS::Darwin, Arch::Aarch64) => {
                format!("WasmEdge-{version}-darwin_arm64.tar.gz")
            }
            (OS::Windows, _) => {
                format!("WasmEdge-{version}-windows.zip")
            }
        }
    }

    fn format_install_name(version: &Version, os: &TargetOS) -> String {
        use TargetOS as OS;

        match os {
            OS::Linux | OS::Ubuntu => format!("WasmEdge-{version}-Linux"),
            OS::Darwin => format!("WasmEdge-{version}-Darwin"),
            OS::Windows => format!("WasmEdge-{version}-Windows"),
        }
    }
}

static MANYLINUX2014_SUPPORTED_VERSIONS: OnceLock<VersionReq> = OnceLock::new();

fn is_manylinux2014_supported(version: &Version) -> bool {
    let req = MANYLINUX2014_SUPPORTED_VERSIONS.get_or_init(|| VersionReq {
        comparators: vec![Comparator {
            op: semver::Op::LessEq,
            major: 0,
            minor: Some(14),
            patch: None,
            pre: Prerelease::EMPTY,
        }],
    });

    req.matches(version)
}

fn is_arm_ubuntu_supported(version: &Version) -> bool {
    // ARM-based Ubuntu 20.04 is supported after 0.13.5
    version >= &Version::new(0, 13, 5)
}

fn download_progress_bar(size: u64) -> ProgressBar {
    let pb = ProgressBar::new(size);
    pb.set_style(
        ProgressStyle::with_template(
            "{spinner:.green} [{elapsed_precise}] [{wide_bar:.cyan/blue}] {bytes}/{total_bytes} ({eta})",
        )
        .expect("progress bar template is valid")
        .with_key("eta", |state: &ProgressState, w: &mut dyn Write| {
            let _ = write!(w, "{:.1}s", state.eta().as_secs_f64());
        })
        .progress_chars("#>-"),
    );

    pb
}

pub fn latest_installed_version(versions_dir: &Path) -> Result<Option<Version>> {
    if !versions_dir.exists() {
        return Ok(None);
    }

    let mut versions = Vec::new();
    for entry in std::fs::read_dir(versions_dir)? {
        let entry = entry?;
        if entry.file_type()?.is_dir() {
            if let Some(name) = entry.file_name().to_str() {
                if let Ok(ver) = Version::parse(name) {
                    versions.push(ver);
                }
            }
        }
    }

    versions.sort_by(|a, b| b.cmp(a));
    Ok(versions.into_iter().next())
}

pub fn runtime_ge_015(runtime: &str) -> bool {
    semver::Version::parse(runtime)
        .map(|v| v >= semver::Version::new(0, 15, 0))
        .unwrap_or(true)
}

/// Metadata describing a single plugin release asset as published on
/// GitHub's release API.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginAssetInfo {
    pub plugin: String,
    pub version: String,
    pub platform: String,
}

const PLUGIN_ASSET_PREFIX: &str = "WasmEdge-plugin-";
const PLUGIN_TAR_GZ: &str = ".tar.gz";
const PLUGIN_ZIP: &str = ".zip";

/// Parse a plugin archive filename like
/// `WasmEdge-plugin-wasi_nn-ggml-0.15.0-manylinux_2_28_x86_64.tar.gz` into
/// `(plugin, version, platform)`. The `tag` parameter disambiguates where
/// the plugin-name portion ends (since plugin names may themselves contain
/// dashes, e.g. `wasi-logging`).
fn parse_plugin_asset_name(name: &str, tag: &str) -> Option<(String, String, String)> {
    let rest = name.strip_prefix(PLUGIN_ASSET_PREFIX)?;
    let needle = format!("-{tag}-");
    let idx = rest.find(&needle)?;
    let plugin = &rest[..idx];
    let plat_with_ext = &rest[idx + needle.len()..];
    let platform = plat_with_ext
        .strip_suffix(PLUGIN_TAR_GZ)
        .or_else(|| plat_with_ext.strip_suffix(PLUGIN_ZIP))
        .unwrap_or(plat_with_ext);
    Some((plugin.to_string(), tag.to_string(), platform.to_string()))
}

/// Build the download URL for a plugin archive on GitHub releases.
///
/// Produces `{base}/{runtime}/WasmEdge-plugin-{plugin}-{runtime}-{platform}.{ext}`
/// where `ext` is `.zip` when `is_zip` is `true`, else `.tar.gz`. Callers
/// pick the extension themselves: the runtime installer maps it from the
/// host OS (Windows → zip, others → tar.gz), but the `plugin list --all`
/// probe builds *both* variants per host to discover whichever exists on
/// the release.
pub fn plugin_asset_url(plugin: &str, runtime: &str, platform: &str, is_zip: bool) -> Result<Url> {
    let ext = if is_zip { "zip" } else { "tar.gz" };
    let filename = format!("{PLUGIN_ASSET_PREFIX}{plugin}-{runtime}-{platform}.{ext}");
    let mut url = Url::parse(WASMEDGE_RELEASE_BASE_URL)
        .expect("WASMEDGE_RELEASE_BASE_URL must be a valid URL");
    url.path_segments_mut()
        .expect("base is valid URL")
        .extend(&[runtime, &filename]);
    Ok(url)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(s: &str) -> Version {
        Version::parse(s).expect("valid semver")
    }

    #[test]
    fn archive_name_ubuntu_x86_64() {
        let a = Asset::new(&v("0.15.0"), &TargetOS::Ubuntu, &TargetArch::X86_64);
        assert_eq!(a.archive_name, "WasmEdge-0.15.0-ubuntu20.04_x86_64.tar.gz");
    }

    #[test]
    fn archive_name_ubuntu_aarch64_requires_0_13_5() {
        let old = Asset::new(&v("0.13.4"), &TargetOS::Ubuntu, &TargetArch::Aarch64);
        assert_eq!(
            old.archive_name, "WasmEdge-0.13.4-manylinux2014_aarch64.tar.gz",
            "pre-0.13.5 aarch64 Ubuntu should fall back to manylinux"
        );

        let new = Asset::new(&v("0.13.5"), &TargetOS::Ubuntu, &TargetArch::Aarch64);
        assert_eq!(
            new.archive_name, "WasmEdge-0.13.5-ubuntu20.04_aarch64.tar.gz",
            "0.13.5+ aarch64 Ubuntu uses ubuntu20.04 asset"
        );
    }

    #[test]
    fn archive_name_linux_manylinux_split_on_0_15() {
        let old = Asset::new(&v("0.14.1"), &TargetOS::Linux, &TargetArch::X86_64);
        assert_eq!(
            old.archive_name, "WasmEdge-0.14.1-manylinux2014_x86_64.tar.gz",
            "<= 0.14 uses manylinux2014"
        );

        let new = Asset::new(&v("0.15.0"), &TargetOS::Linux, &TargetArch::X86_64);
        assert_eq!(
            new.archive_name, "WasmEdge-0.15.0-manylinux_2_28_x86_64.tar.gz",
            ">= 0.15 uses manylinux_2_28"
        );
    }

    #[test]
    fn archive_name_darwin_uses_arm64_alias() {
        let a = Asset::new(&v("0.15.0"), &TargetOS::Darwin, &TargetArch::Aarch64);
        assert_eq!(a.archive_name, "WasmEdge-0.15.0-darwin_arm64.tar.gz");

        let x = Asset::new(&v("0.15.0"), &TargetOS::Darwin, &TargetArch::X86_64);
        assert_eq!(x.archive_name, "WasmEdge-0.15.0-darwin_x86_64.tar.gz");
    }

    #[test]
    fn archive_name_windows_is_zip() {
        let a = Asset::new(&v("0.15.0"), &TargetOS::Windows, &TargetArch::X86_64);
        assert_eq!(a.archive_name, "WasmEdge-0.15.0-windows.zip");
    }

    #[test]
    fn install_name_per_os() {
        let lin = Asset::new(&v("0.15.0"), &TargetOS::Linux, &TargetArch::X86_64);
        assert_eq!(lin.install_name, "WasmEdge-0.15.0-Linux");
        let ubu = Asset::new(&v("0.15.0"), &TargetOS::Ubuntu, &TargetArch::X86_64);
        assert_eq!(ubu.install_name, "WasmEdge-0.15.0-Linux");
        let mac = Asset::new(&v("0.15.0"), &TargetOS::Darwin, &TargetArch::Aarch64);
        assert_eq!(mac.install_name, "WasmEdge-0.15.0-Darwin");
        let win = Asset::new(&v("0.15.0"), &TargetOS::Windows, &TargetArch::X86_64);
        assert_eq!(win.install_name, "WasmEdge-0.15.0-Windows");
    }

    #[test]
    fn manylinux2014_supported_boundary() {
        assert!(is_manylinux2014_supported(&v("0.13.0")));
        assert!(is_manylinux2014_supported(&v("0.14.99")));
        assert!(!is_manylinux2014_supported(&v("0.15.0")));
        assert!(!is_manylinux2014_supported(&v("1.0.0")));
    }

    #[test]
    fn arm_ubuntu_supported_boundary() {
        assert!(!is_arm_ubuntu_supported(&v("0.13.4")));
        assert!(is_arm_ubuntu_supported(&v("0.13.5")));
        assert!(is_arm_ubuntu_supported(&v("0.15.0")));
    }

    #[test]
    fn asset_url_is_valid() {
        let a = Asset::new(&v("0.15.0"), &TargetOS::Linux, &TargetArch::X86_64);
        let url = a.url().expect("url builds");
        assert_eq!(
            url.as_str(),
            "https://github.com/WasmEdge/WasmEdge/releases/download/0.15.0/WasmEdge-0.15.0-manylinux_2_28_x86_64.tar.gz"
        );
    }

    #[test]
    fn latest_installed_version_missing_dir_is_none() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let missing = tmp.path().join("nope");
        assert_eq!(latest_installed_version(&missing).unwrap(), None);
    }

    #[test]
    fn latest_installed_version_empty_dir_is_none() {
        let tmp = tempfile::tempdir().expect("tempdir");
        assert_eq!(latest_installed_version(tmp.path()).unwrap(), None);
    }

    #[test]
    fn latest_installed_version_picks_highest_semver() {
        let tmp = tempfile::tempdir().expect("tempdir");
        for name in ["0.14.1", "0.15.0", "0.13.5", "not-a-version"] {
            std::fs::create_dir(tmp.path().join(name)).expect("mkdir");
        }
        // Also drop a regular file to ensure it's ignored.
        std::fs::write(tmp.path().join("stray.txt"), "").expect("file");

        let picked = latest_installed_version(tmp.path()).unwrap();
        assert_eq!(picked, Some(v("0.15.0")));
    }

    #[test]
    fn runtime_ge_015_boundaries() {
        assert!(!runtime_ge_015("0.14.99"));
        assert!(runtime_ge_015("0.15.0"));
        // semver ordering: prereleases sort before the stable release.
        assert!(
            !runtime_ge_015("0.15.0-rc.1"),
            "0.15.0-rc.1 < 0.15.0 per semver rules",
        );
        assert!(runtime_ge_015("1.0.0"));
        // Unparseable input fails open (defensive).
        assert!(runtime_ge_015("not-a-version"));
    }

    #[test]
    fn parse_plugin_asset_name_targz() {
        let parsed = parse_plugin_asset_name(
            "WasmEdge-plugin-wasi_nn-ggml-0.15.0-manylinux_2_28_x86_64.tar.gz",
            "0.15.0",
        );
        assert_eq!(
            parsed,
            Some((
                "wasi_nn-ggml".to_string(),
                "0.15.0".to_string(),
                "manylinux_2_28_x86_64".to_string(),
            ))
        );
    }

    #[test]
    fn parse_plugin_asset_name_zip() {
        let parsed = parse_plugin_asset_name(
            "WasmEdge-plugin-wasi_crypto-0.14.1-windows_x86_64.zip",
            "0.14.1",
        );
        assert_eq!(
            parsed,
            Some((
                "wasi_crypto".to_string(),
                "0.14.1".to_string(),
                "windows_x86_64".to_string(),
            ))
        );
    }

    #[test]
    fn parse_plugin_asset_name_rejects_unrelated_prefix() {
        assert_eq!(
            parse_plugin_asset_name("WasmEdge-0.15.0-linux.tar.gz", "0.15.0"),
            None
        );
    }

    #[test]
    fn parse_plugin_asset_name_rejects_tag_mismatch() {
        // The needle "-0.15.0-" is not present in an archive tagged 0.14.1.
        assert_eq!(
            parse_plugin_asset_name(
                "WasmEdge-plugin-wasi_nn-ggml-0.14.1-manylinux2014_x86_64.tar.gz",
                "0.15.0"
            ),
            None
        );
    }

    #[test]
    fn parse_plugin_asset_name_handles_dashed_plugin_names() {
        let parsed = parse_plugin_asset_name(
            "WasmEdge-plugin-wasi-logging-0.15.0-darwin_arm64.tar.gz",
            "0.15.0",
        );
        assert_eq!(
            parsed,
            Some((
                "wasi-logging".to_string(),
                "0.15.0".to_string(),
                "darwin_arm64".to_string(),
            ))
        );
    }

    #[test]
    fn plugin_asset_url_targz() {
        let url = plugin_asset_url("wasi_nn-ggml", "0.15.0", "manylinux_2_28_x86_64", false)
            .expect("url builds");
        assert_eq!(
            url.as_str(),
            "https://github.com/WasmEdge/WasmEdge/releases/download/0.15.0/WasmEdge-plugin-wasi_nn-ggml-0.15.0-manylinux_2_28_x86_64.tar.gz"
        );
    }

    #[test]
    fn plugin_asset_url_zip() {
        let url =
            plugin_asset_url("wasi_crypto", "0.14.1", "windows_x86_64", true).expect("url builds");
        assert_eq!(
            url.as_str(),
            "https://github.com/WasmEdge/WasmEdge/releases/download/0.14.1/WasmEdge-plugin-wasi_crypto-0.14.1-windows_x86_64.zip"
        );
    }
}

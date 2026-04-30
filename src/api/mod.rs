use std::{
    fmt::Write,
    io::{Read, Seek},
    path::Path,
    sync::OnceLock,
};

use crate::{
    constants::{
        CHECKSUM_FILE_NAME, DEFAULT_CONNECT_TIMEOUT_SECS, DEFAULT_REQUEST_TIMEOUT_SECS,
        DOWNLOAD_BUFFER_SIZE, WASMEDGE_GIT_URL, WASMEDGE_RELEASE_BASE_URL,
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

        download_asset(no_progress, response, &mut async_file).await?;
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
async fn download_asset(
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
}

use std::{
    fmt::Write,
    io::{Read, Seek},
    path::Path,
    sync::OnceLock,
};

use crate::{
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

const WASM_EDGE_GIT_URL: &str = "https://github.com/WasmEdge/WasmEdge.git";
const WASM_EDGE_RELEASE_ASSET_BASE_URL: &str =
    "https://github.com/WasmEdge/WasmEdge/releases/download";
const CHECKSUM_FILE_NAME: &str = "SHA256SUM";
const BUFFER_SIZE: usize = 8 * 1024; // 8KB

impl WasmEdgeApiClient {
    fn http_client(&self) -> Client {
        reqwest::ClientBuilder::new()
            .connect_timeout(std::time::Duration::from_secs(self.connect_timeout))
            .timeout(std::time::Duration::from_secs(self.request_timeout))
            .user_agent(format!(
                "wasmedgeup/{} (+https://github.com/WasmEdge/wasmedgeup)",
                env!("CARGO_PKG_VERSION")
            ))
            .build()
            .expect("Failed to build reqwest client")
    }

    pub fn releases(&self, filter: ReleasesFilter, num_releases: usize) -> Result<Vec<Version>> {
        let releases = releases::get_all(WASM_EDGE_GIT_URL, filter)?;
        Ok(releases.into_iter().take(num_releases).collect())
    }

    pub fn latest_release(&self) -> Result<Version> {
        let releases = releases::get_all(WASM_EDGE_GIT_URL, ReleasesFilter::Stable)?;
        releases.into_iter().next().ok_or(Error::Unknown)
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

        let client = self.http_client();
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
        let mut url = Url::parse(WASM_EDGE_RELEASE_ASSET_BASE_URL)
            .expect("WASM_EDGE_RELEASE_ASSET_BASE_URL must be a valid URL");

        url.path_segments_mut()
            .expect("base is valid URL")
            .extend(&[&version.to_string(), CHECKSUM_FILE_NAME]);

        tracing::debug!(%url, CHECKSUM_FILE_NAME, "Trying checksum file");

        let client = self.http_client();
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
        let mut buffer = vec![0; BUFFER_SIZE];

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
            connect_timeout: 15, // 15 seconds for connection
            request_timeout: 90, // 90 seconds for request
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
        let mut url = Url::parse(WASM_EDGE_RELEASE_ASSET_BASE_URL)
            .expect("WASM_EDGE_RELEASE_ASSET_BASE_URL must be a valid URL");

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
    pb.set_style(ProgressStyle::with_template("{spinner:.green} [{elapsed_precise}] [{wide_bar:.cyan/blue}] {bytes}/{total_bytes} ({eta})")
        .unwrap()
        .with_key("eta", |state: &ProgressState, w: &mut dyn Write| write!(w, "{:.1}s", state.eta().as_secs_f64()).unwrap())
        .progress_chars("#>-"));

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

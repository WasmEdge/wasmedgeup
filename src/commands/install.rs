use std::{
    fmt::Write,
    path::{Path, PathBuf},
    sync::OnceLock,
};

use clap::Parser;
use indicatif::{ProgressBar, ProgressState, ProgressStyle};
use reqwest::Response;
use semver::{Comparator, Prerelease, Version, VersionReq};
use snafu::ResultExt;
use tokio::{fs::File, io::AsyncWriteExt};
use tracing::Instrument;
use url::Url;

use crate::{
    cli::{CommandContext, CommandExecutor},
    prelude::*,
    target::{TargetArch, TargetOS},
};

const WASM_EDGE_RELEASE_ASSET_BASE_URL: &str =
    "https://github.com/WasmEdge/WasmEdge/releases/download/";

fn default_path() -> PathBuf {
    let home_dir = dirs::home_dir().expect("home_dir should be present");
    home_dir.join(".wasmedge")
}

fn default_tmpdir() -> PathBuf {
    std::env::temp_dir()
}

#[derive(Debug, Parser)]
pub struct InstallArgs {
    /// WasmEdge version to install, e.g. `latest`, `0.14.1`, `0.14.1-rc.1`, etc.
    pub version: String,

    /// Set the install location for the WasmEdge runtime
    ///
    /// Defaults to `$HOME/.wasmedge` on Unix-like systems and `%HOME%\.wasmedge` on Windows.
    #[arg(short, long)]
    pub path: Option<PathBuf>,

    /// Set the temporary directory for staging downloaded assets
    ///
    /// Defaults to the system temporary directory, this differs between operating systems.
    #[arg(short, long)]
    pub tmpdir: Option<PathBuf>,

    /// Set the target OS for the WasmEdge runtime
    ///
    /// `wasmedgeup` will detect the OS of your host system by default.
    #[arg(short, long)]
    pub os: Option<TargetOS>,

    /// Set the target architecture for the WasmEdge runtime
    ///
    /// `wasmedgeup` will detect the architecture of your host system by default.
    #[arg(short, long)]
    pub arch: Option<TargetArch>,
}

impl CommandExecutor for InstallArgs {
    async fn execute(mut self, ctx: CommandContext) -> Result<()> {
        let version = match self.version.as_str() {
            "latest" => ctx.client.latest_release()?,
            v => Version::parse(v).context(SemVerSnafu {})?,
        };

        let os = self.os.get_or_insert_default();
        let arch = self.arch.get_or_insert_default();
        tracing::debug!(?os, ?arch, "Host OS and architecture detected");
        let asset = Asset::new(&version, os, arch);

        let mut response = self
            .fetch_release_asset(&version, &asset.archive_name)
            .await?;

        let tmpdir = self.tmpdir.unwrap_or_else(default_tmpdir);
        let tmpfile = tempfile::tempfile_in(&tmpdir).unwrap();
        let mut file = File::from_std(tmpfile.try_clone()?);

        let pb = if ctx.no_progress {
            None
        } else {
            Some(download_progress_bar(
                response.content_length().unwrap_or_default(),
            ))
        };

        while let Some(mut chunk) = response
            .chunk()
            .await
            .context(RequestSnafu { resource: "chunk" })?
        {
            if let Some(ref pb) = pb {
                pb.inc(chunk.len() as u64)
            }
            file.write_buf(&mut chunk).await?;
        }

        file.flush().await?;
        if let Some(ref pb) = pb {
            pb.finish_and_clear()
        }

        tracing::debug!(dest = %tmpdir.display(), "Start unpacking");
        Self::extract_asset_archive(tmpfile.try_clone()?, &tmpdir).await?;

        // Try with `tmpdir/WasmEdge-{version}-{os}` first, and if it's not a directory, fallback
        // to `tmpdir`
        // (both patterns are used in WasmEdge)
        let mut extracted_dir = tmpdir.join(&asset.install_name);
        if !extracted_dir.is_dir() {
            extracted_dir = tmpdir;
        }

        let target_dir = self.path.unwrap_or_else(default_path);
        tracing::debug!(extracted_dir = %extracted_dir.display(), target_dir = %target_dir.display(), "Start copying files to target location");

        crate::fs::copy_tree(&extracted_dir, &target_dir).await;

        Ok(())
    }
}

impl InstallArgs {
    async fn fetch_release_asset(
        &mut self,
        version: &Version,
        archive_name: &str,
    ) -> Result<Response> {
        // This should never fail
        let url = Url::parse(WASM_EDGE_RELEASE_ASSET_BASE_URL).unwrap();
        let url = url.join(&format!("{}/", version)).context(UrlSnafu {})?;

        let url = url.join(archive_name).context(UrlSnafu {})?;

        let span = tracing::debug_span!("sending_request", %url);

        let response = reqwest::get(url)
            .instrument(span)
            .await
            .context(RequestSnafu { resource: "assets" })?;

        Ok(response)
    }

    async fn extract_asset_archive(mut file: std::fs::File, dest: &Path) -> Result<()> {
        use std::io::Seek;
        use tokio::fs;

        file.rewind()?;
        fs::create_dir_all(dest).await?;
        Self::extract_to(file, dest)?;

        Ok(())
    }

    #[cfg(unix)]
    fn extract_to(file: std::fs::File, to: &Path) -> Result<()> {
        use flate2::read::GzDecoder;
        use tar::Archive;

        let tar = GzDecoder::new(file);
        let mut archive = Archive::new(tar);
        archive.unpack(to).context(ExtractSnafu {})?;

        Ok(())
    }

    #[cfg(windows)]
    fn extract_to(file: std::fs::File, to: &Path) -> Result<()> {
        use zip::ZipArchive;

        let mut archive = ZipArchive::new(file).context(ExtractSnafu {})?;
        archive.extract(to).context(ExtractSnafu {})?;

        Ok(())
    }
}

fn download_progress_bar(size: u64) -> ProgressBar {
    let pb = ProgressBar::new(size);
    pb.set_style(ProgressStyle::with_template("{spinner:.green} [{elapsed_precise}] [{wide_bar:.cyan/blue}] {bytes}/{total_bytes} ({eta})")
        .unwrap()
        .with_key("eta", |state: &ProgressState, w: &mut dyn Write| write!(w, "{:.1}s", state.eta().as_secs_f64()).unwrap())
        .progress_chars("#>-"));

    pb
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

struct Asset {
    install_name: String,
    archive_name: String,
}

impl Asset {
    fn new(version: &Version, os: &TargetOS, arch: &TargetArch) -> Self {
        use TargetArch as Arch;
        use TargetOS as OS;

        let archive_name = match (os, arch) {
            (OS::Ubuntu, Arch::X86_64) => {
                format!("WasmEdge-{}-ubuntu20.04_x86_64.tar.gz", version)
            }
            // ARM-based Ubuntu 20.04 is supported after 0.13.5
            (OS::Ubuntu, Arch::Aarch64) if version >= &Version::new(0, 13, 5) => {
                format!("WasmEdge-{}-ubuntu20.04_aarch64.tar.gz", version)
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
                format!("WasmEdge-{}-{}_{}.tar.gz", version, manylinux_version, arch)
            }
            (OS::Darwin, Arch::X86_64) => {
                format!("WasmEdge-{}-darwin_x86_64.tar.gz", version)
            }
            (OS::Darwin, Arch::Aarch64) => {
                format!("WasmEdge-{}-darwin_arm64.tar.gz", version)
            }
            (OS::Windows, _) => {
                format!("WasmEdge-{}-windows.zip", version)
            }
        };

        let install_name = match os {
            OS::Linux | OS::Ubuntu => format!("WasmEdge-{}-Linux", version),
            OS::Darwin => format!("WasmEdge-{}-Darwin", version),
            OS::Windows => format!("WasmEdge-{}-Windows", version),
        };

        Self {
            archive_name,
            install_name,
        }
    }
}

use std::path::PathBuf;

use clap::Parser;
use semver::Version;
use snafu::ResultExt;

use crate::{
    api::{Asset, WasmEdgeApiClient},
    cli::{CommandContext, CommandExecutor},
    fs,
    prelude::*,
    shell_utils,
    target::{TargetArch, TargetOS},
};

fn default_path() -> PathBuf {
    dirs::home_dir()
        .expect("home_dir should be present")
        .join(".wasmedge")
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
    /// Defaults to the system temporary directory.
    #[arg(short, long)]
    pub tmpdir: Option<PathBuf>,

    /// Set the target OS for the WasmEdge runtime
    ///
    /// Detected automatically if not specified.
    #[arg(short, long)]
    pub os: Option<TargetOS>,

    /// Set the target architecture for the WasmEdge runtime
    ///
    /// Detected automatically if not specified.
    #[arg(short, long)]
    pub arch: Option<TargetArch>,
}

impl CommandExecutor for InstallArgs {
    #[tracing::instrument(name = "install", skip_all, fields(version = self.version))]
    async fn execute(mut self, ctx: CommandContext) -> Result<()> {
        let version = self.resolve_version(&ctx.client).inspect_err(
            |e| tracing::error!(error = %e.to_string(), "Failed to resolve version"),
        )?;
        tracing::debug!(%version, "Resolved version for installation");

        let os = self.os.get_or_insert_default();
        let arch = self.arch.get_or_insert_default();
        tracing::debug!(?os, ?arch, "Host OS and architecture detected");

        let asset = Asset::new(&version, os, arch);
        let tmpdir = self.tmpdir.unwrap_or_else(default_tmpdir);

        let file = ctx
            .client
            .download_asset(&asset, &tmpdir, ctx.no_progress)
            .await
            .inspect_err(|e| tracing::error!(error = %e.to_string(), "Failed to download asset"))?;

        tracing::debug!(file_path = %file.path().display(), dest = %tmpdir.display(), "Starting extraction of asset");
        fs::extract_archive(file.into_file(), &tmpdir)
            .await
            .inspect_err(|e| tracing::error!(error = %e.to_string(), "Failed to extract asset"))?;
        tracing::debug!(dest = %tmpdir.display(), "Extraction completed successfully");

        let mut extracted_dir = tmpdir.join(&asset.install_name);
        if !extracted_dir.is_dir() {
            tracing::debug!(extracted_dir = %extracted_dir.display(), "Falling back to tmpdir as extracted directory");
            extracted_dir = tmpdir.clone();
        }

        let target_dir = self.path.unwrap_or_else(default_path);
        tracing::debug!(extracted_dir = %extracted_dir.display(), target_dir = %target_dir.display(), "Start copying files to target location");

        fs::copy_tree(&extracted_dir, &target_dir).await;
        tracing::debug!(target_dir = %target_dir.display(), "Copying files completed");

        let install_dir = target_dir.join("bin");
        shell_utils::setup_path(&install_dir)?;

        Ok(())
    }
}

impl InstallArgs {
    fn resolve_version(&self, client: &WasmEdgeApiClient) -> Result<Version> {
        if self.version == "latest" {
            client.latest_release()
        } else {
            Version::parse(&self.version).context(SemVerSnafu {})
        }
    }
}

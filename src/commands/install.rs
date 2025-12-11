use std::path::PathBuf;

use clap::Parser;
use tokio::fs;

use crate::{
    api::{Asset, WasmEdgeApiClient},
    cli::{CommandContext, CommandExecutor},
    commands::default_path,
    prelude::*,
    shell_utils,
    target::{TargetArch, TargetOS},
};

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

    /// Skip checksum retrieval and verification for the downloaded asset
    ///
    /// This option disables integrity verification.
    #[arg(long)]
    pub no_verify: bool,
}

impl CommandExecutor for InstallArgs {
    /// Executes the installation process by resolving the version, downloading the asset,
    /// unpacking it, and copying the extracted files to the target directory.
    ///
    /// # Steps:
    /// 1. Resolves the version (either a specific version or the latest).
    /// 2. Downloads the asset for the appropriate OS and architecture.
    /// 3. Unpacks the asset to a temporary directory.
    /// 4. Copies the extracted files to the target directory.
    /// 5. Add the installed bin directory to PATH
    ///
    /// # Arguments
    ///
    /// * `ctx` - The command context containing the client and progress bar settings.
    ///
    /// # Errors
    ///
    /// Returns an error if any step fails, such as download failure, extraction issues,
    /// or copying issues.
    #[tracing::instrument(name = "install", skip_all, fields(version = self.version))]
    async fn execute(mut self, ctx: CommandContext) -> Result<()> {
        let version = ctx.client.resolve_version(&self.version).inspect_err(
            |e| tracing::error!(error = %e.to_string(), "Failed to resolve version"),
        )?;
        tracing::debug!(%version, "Resolved version for installation");

        let os = self.os.get_or_insert_default();
        let arch = self.arch.get_or_insert_default();
        tracing::debug!(?os, ?arch, "Host OS and architecture detected");

        let asset = Asset::new(&version, os, arch);

        // Create a dedicated temporary workspace for this installation. This provides isolation
        // between concurrent installations and ensures consistent handling of different archive
        // structures. The source path for copying will be either:
        //   - /tmp/WasmEdge-version-os/ (for archives with root-level files)
        //   - /tmp/WasmEdge-version-os/WasmEdge-version-os/ (for nested archives)
        let tmpdir = self
            .tmpdir
            .unwrap_or_else(default_tmpdir)
            .join(&asset.install_name);
        fs::create_dir_all(&tmpdir).await.inspect_err(
            |e| tracing::error!(error = %e.to_string(), "Failed to create temporary directory"),
        )?;
        tracing::debug!(tmpdir = %tmpdir.display(), "Created temporary directory");

        let mut file = ctx
            .client
            .download_asset(&asset, &tmpdir, ctx.no_progress)
            .await
            .inspect_err(|e| tracing::error!(error = %e.to_string(), "Failed to download asset"))?
            .into_file();

        if self.no_verify {
            tracing::warn!("Skipping checksum retrieval and verification due to --no-verify flag");
        } else {
            let expected_checksum = ctx
                .client
                .get_release_checksum(&version, &asset)
                .await
                .inspect_err(
                    |e| tracing::error!(error = %e.to_string(), "Failed to get checksum"),
                )?;
            tracing::debug!(%expected_checksum, "Got release checksum");

            WasmEdgeApiClient::verify_file_checksum(&mut file, &expected_checksum)
                .await
                .inspect_err(
                    |e| tracing::error!(error = %e.to_string(), "Checksum verification failed"),
                )?;
            tracing::debug!("Checksum verified successfully");
        }

        tracing::debug!(dest = %tmpdir.display(), "Starting extraction of asset");
        crate::fs::extract_archive(&mut file, &tmpdir)
            .await
            .inspect_err(|e| tracing::error!(error = %e.to_string(), "Failed to extract asset"))?;
        tracing::debug!(dest = %tmpdir.display(), "Extraction completed successfully");

        let target_dir = match self.path {
            Some(p) => p,
            None => default_path()?,
        };

        if target_dir.exists() {
            if crate::fs::can_write_to_directory(&target_dir) {
                tracing::debug!(target_dir = %target_dir.display(), "Verified write permissions");
            } else {
                return Err(crate::commands::insufficient_permissions(
                    &target_dir,
                    "write to target directory",
                    &version.to_string(),
                ));
            }
        } else {
            match fs::create_dir_all(&target_dir).await {
                Ok(_) => {
                    if !crate::fs::can_write_to_directory(&target_dir) {
                        tracing::debug!(path = %target_dir.display(), "Created directory but cannot write to it");
                        return Err(crate::commands::insufficient_permissions(
                            &target_dir,
                            "write to target directory",
                            &version.to_string(),
                        ));
                    }
                    tracing::debug!(target_dir = %target_dir.display(), "Created target directory");
                }
                Err(e) => {
                    tracing::debug!(error = %e, path = %target_dir.display(), "Failed to create directory");
                    return Err(crate::commands::insufficient_permissions(
                        &target_dir,
                        "create directory",
                        &version.to_string(),
                    ));
                }
            }
        }

        let version_dir = target_dir.join("versions").join(version.to_string());
        fs::create_dir_all(&version_dir).await.inspect_err(
            |e| tracing::error!(error = %e.to_string(), "Failed to create version directory"),
        )?;
        tracing::debug!(version_dir = %version_dir.display(), "Created version directory");

        let mut read_dir = fs::read_dir(&tmpdir).await?;
        let mut source_dir = tmpdir.clone();

        if let Some(entry) = read_dir.next_entry().await? {
            let file_name = entry.file_name().into_string().unwrap_or_default();
            if file_name.starts_with("WasmEdge-") && entry.file_type().await?.is_dir() {
                source_dir = entry.path();
            } else if !matches!(file_name.as_str(), "bin" | "lib64" | "include" | "lib") {
                tracing::debug!(found_file = %file_name, "Unexpected file found in archive");
                return Err(Error::InvalidArchiveStructure {
                    found_file: file_name,
                });
            }
        } else {
            tracing::debug!(dir = %tmpdir.display(), "Archive directory is empty");
            return Err(Error::InvalidArchiveStructure {
                found_file: "<empty directory>".to_string(),
            });
        }

        tracing::debug!(source_dir = %source_dir.display(), "Start copying files to version directory");
        crate::fs::copy_tree(&source_dir, &version_dir).await?;
        tracing::debug!(version_dir = %version_dir.display(), "Copying files to version directory completed");

        fs::remove_dir_all(&tmpdir).await.inspect_err(
            |e| tracing::error!(error = %e.to_string(), "Failed to clean up temporary directory"),
        )?;
        tracing::debug!(tmpdir = %tmpdir.display(), "Cleaned up temporary directory");

        tracing::debug!("Creating version symlinks");
        crate::fs::create_version_symlinks(&target_dir, &version.to_string()).await?;
        shell_utils::setup_path(&target_dir)?;

        println!(
            "Installed WasmEdge {version}\nInstall root: {}",
            target_dir.display()
        );

        Ok(())
    }
}

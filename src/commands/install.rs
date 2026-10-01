use std::{
    fs::OpenOptions,
    io::{ErrorKind, Read, Write},
    path::{Path, PathBuf},
};

use clap::Parser;
use tokio::fs;

use crate::{
    api::{Asset, WasmEdgeApiClient},
    cli::{CommandContext, CommandExecutor},
    commands::{normalize_absolute_path, resolve_install_path},
    constants::{VERSION_INSTALL_MARKER, VERSION_INSTALL_MARKER_CONTENT},
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
        let version = ctx
            .client
            .resolve_version(&self.version)
            .await
            .inspect_err(
                |e| tracing::error!(error = %e.to_string(), "Failed to resolve version"),
            )?;
        tracing::debug!(%version, "Resolved version for installation");

        let os = self.os.get_or_insert_default();
        let arch = self.arch.get_or_insert_default();
        tracing::debug!(?os, ?arch, "Host OS and architecture detected");

        let asset = Asset::new(&version, os, arch);

        // Stage this installation in an isolated temporary workspace with a
        // randomized name (see `create_temp_workspace`) for isolation between
        // concurrent installs and consistent handling of archive structures.
        // The source path for copying is either:
        //   - <workspace>/ (for archives with root-level files)
        //   - <workspace>/WasmEdge-<version>-<os>/ (for nested archives)
        let tmpbase = self.tmpdir.unwrap_or_else(default_tmpdir);
        let workspace = crate::fs::create_temp_workspace(&tmpbase, &asset.install_name)
            .inspect_err(
                |e| tracing::error!(error = %e.to_string(), "Failed to create temporary workspace"),
            )?;
        let tmpdir = workspace.path().to_path_buf();
        tracing::debug!(tmpdir = %tmpdir.display(), "Created temporary workspace directory");

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
        crate::fs::extract_archive(file, &tmpdir)
            .await
            .inspect_err(|e| tracing::error!(error = %e.to_string(), "Failed to extract asset"))?;
        tracing::debug!(dest = %tmpdir.display(), "Extraction completed successfully");

        let target_dir = normalize_absolute_path(&resolve_install_path(self.path)?)?;

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

        let versions_dir = target_dir.join("versions");
        fs::create_dir_all(&versions_dir).await.inspect_err(
            |e| tracing::error!(error = %e.to_string(), "Failed to create versions directory"),
        )?;
        let version_dir = versions_dir.join(version.to_string());
        claim_version_directory(&version_dir)?;
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

        // The runtime is already copied into `version_dir`, so failing to remove
        // the staging workspace must not abort the install or skip the symlink/
        // PATH setup below. Log and continue, leaving the dir for the temp reaper.
        if let Err(e) = workspace.close() {
            tracing::warn!(error = %e.to_string(), tmpdir = %tmpdir.display(), "Failed to clean up temporary workspace; continuing");
        }

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

fn claim_version_directory(version_dir: &Path) -> Result<()> {
    let created = match std::fs::create_dir(version_dir) {
        Ok(()) => true,
        Err(error) if error.kind() == ErrorKind::AlreadyExists => false,
        Err(error) => return Err(error.into()),
    };

    if !created {
        let metadata = std::fs::symlink_metadata(version_dir)?;
        if metadata.file_type().is_symlink() || !metadata.is_dir() {
            return Err(Error::InvalidPath {
                path: version_dir.display().to_string(),
                reason: "the version path must be a real directory, not a symlink or file"
                    .to_string(),
            });
        }

        let marker_is_valid = valid_version_marker(version_dir);
        let has_runtime = ["bin/wasmedge", "bin/wasmedge.exe"]
            .into_iter()
            .any(|binary| {
                std::fs::symlink_metadata(version_dir.join(binary))
                    .is_ok_and(|metadata| metadata.is_file() && !metadata.file_type().is_symlink())
            });
        if !marker_is_valid && !has_runtime {
            return Err(Error::InvalidPath {
                path: version_dir.display().to_string(),
                reason: "refusing to claim an existing directory that is not owned by wasmedgeup"
                    .to_string(),
            });
        }
    }

    write_version_marker(version_dir)
}

fn valid_version_marker(version_dir: &Path) -> bool {
    let marker = version_dir.join(VERSION_INSTALL_MARKER);
    let Some(metadata) = std::fs::symlink_metadata(&marker).ok() else {
        return false;
    };
    if metadata.file_type().is_symlink()
        || !metadata.is_file()
        || metadata.len() != VERSION_INSTALL_MARKER_CONTENT.len() as u64
    {
        return false;
    }

    let mut content = Vec::with_capacity(VERSION_INSTALL_MARKER_CONTENT.len() + 1);
    std::fs::File::open(marker)
        .and_then(|file| {
            file.take(VERSION_INSTALL_MARKER_CONTENT.len() as u64 + 1)
                .read_to_end(&mut content)
        })
        .is_ok_and(|_| content == VERSION_INSTALL_MARKER_CONTENT.as_bytes())
}

fn write_version_marker(version_dir: &Path) -> Result<()> {
    let marker = version_dir.join(VERSION_INSTALL_MARKER);
    match OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&marker)
    {
        Ok(mut file) => {
            file.write_all(VERSION_INSTALL_MARKER_CONTENT.as_bytes())?;
            file.sync_data()?;
            Ok(())
        }
        Err(error)
            if error.kind() == ErrorKind::AlreadyExists && valid_version_marker(version_dir) =>
        {
            Ok(())
        }
        Err(error) if error.kind() == ErrorKind::AlreadyExists => Err(Error::InvalidPath {
            path: marker.display().to_string(),
            reason: "the wasmedgeup ownership marker is not a regular marker file".to_string(),
        }),
        Err(error) => Err(error.into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn claim_marks_a_new_version_before_copying() {
        let parent = tempfile::tempdir().unwrap();
        let version_dir = parent.path().join("0.14.1");

        claim_version_directory(&version_dir).unwrap();

        assert_eq!(
            std::fs::read_to_string(version_dir.join(VERSION_INSTALL_MARKER)).unwrap(),
            VERSION_INSTALL_MARKER_CONTENT
        );
    }

    #[test]
    fn claim_migrates_a_legacy_runtime_directory() {
        let parent = tempfile::tempdir().unwrap();
        let version_dir = parent.path().join("0.14.1");
        std::fs::create_dir_all(version_dir.join("bin")).unwrap();
        std::fs::write(version_dir.join("bin/wasmedge"), "runtime").unwrap();

        claim_version_directory(&version_dir).unwrap();

        assert!(valid_version_marker(&version_dir));
    }

    #[test]
    fn claim_preserves_an_existing_unowned_directory() {
        let parent = tempfile::tempdir().unwrap();
        let version_dir = parent.path().join("0.14.1");
        std::fs::create_dir(&version_dir).unwrap();
        std::fs::write(version_dir.join("foreign-data"), "preserve").unwrap();

        assert!(claim_version_directory(&version_dir).is_err());
        assert_eq!(
            std::fs::read_to_string(version_dir.join("foreign-data")).unwrap(),
            "preserve"
        );
        assert!(!version_dir.join(VERSION_INSTALL_MARKER).exists());
    }
}

use std::{
    io::{ErrorKind, Read, Write},
    path::{Path, PathBuf},
};

use cap_primitives::fs::FollowSymlinks;
use cap_std::fs::{Dir, OpenOptions};
use clap::Parser;
use tokio::fs;

use crate::{
    api::{Asset, WasmEdgeApiClient},
    cli::{CommandContext, CommandExecutor},
    commands::resolve_normalized_install_path,
    constants::{
        VERSION_INSTALL_MARKER, VERSION_INSTALL_MARKER_CONTENT, VERSION_STAGING_MARKER,
        VERSION_STAGING_MARKER_CONTENT,
    },
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
        let target_dir = resolve_normalized_install_path(self.path.take())?;
        shell_utils::validate_shell_configuration_path(&target_dir)?;

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

        let install_root = match crate::fs::open_or_create_dir_nofollow(&target_dir) {
            Ok(install_root) => install_root,
            Err(error) if error.kind() == ErrorKind::PermissionDenied => {
                tracing::debug!(%error, path = %target_dir.display(), "Cannot create or open target directory");
                return Err(crate::commands::insufficient_permissions(
                    &target_dir,
                    "create or open target directory",
                    &version.to_string(),
                ));
            }
            Err(error) => {
                return Err(Error::InvalidPath {
                    path: target_dir.display().to_string(),
                    reason: format!(
                        "the install root must be a real directory opened without following \
                         symlinks: {error}"
                    ),
                });
            }
        };
        if !crate::fs::can_write_to_cap_directory(&install_root) {
            tracing::debug!(path = %target_dir.display(), "Cannot write to target directory");
            return Err(crate::commands::insufficient_permissions(
                &target_dir,
                "write to target directory",
                &version.to_string(),
            ));
        }
        tracing::debug!(target_dir = %target_dir.display(), "Verified target directory");

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

        let versions_dir = target_dir.join("versions");
        let versions_root =
            crate::fs::open_or_create_cap_dir_nofollow(&install_root, Path::new("versions"))
                .map_err(|error| Error::InvalidPath {
                    path: versions_dir.display().to_string(),
                    reason: format!(
                        "the versions path must be a real directory opened without following \
                         symlinks: {error}"
                    ),
                })?;
        let version_name = version.to_string();
        let version_dir = versions_dir.join(&version_name);
        let (version_root, staged_version) =
            claim_version_directory(&versions_root, Path::new(&version_name), &version_dir)?;
        tracing::debug!(version_dir = %version_dir.display(), "Claimed version directory");

        tracing::debug!(source_dir = %source_dir.display(), "Start copying files to version directory");
        crate::fs::copy_tree_to_cap_dir(&source_dir, &version_root, &version_dir).await?;
        // Keep the exact directory copied above pinned through publication and
        // activation. On Windows the staging handle itself performs the
        // handle-relative rename; Unix keeps the open directory across rename.
        let pinned_version_root = if let Some(staging) = staged_version {
            publish_version_directory(
                version_root,
                staging,
                &versions_root,
                Path::new(&version_name),
                &version_dir,
            )?
        } else {
            version_root
        };
        tracing::debug!(version_dir = %version_dir.display(), "Copying files to version directory completed");

        // The runtime is already copied into `version_dir`, so failing to remove
        // the staging workspace must not abort the install or skip the symlink/
        // PATH setup below. Log and continue, leaving the dir for the temp reaper.
        if let Err(e) = workspace.close() {
            tracing::warn!(error = %e.to_string(), tmpdir = %tmpdir.display(), "Failed to clean up temporary workspace; continuing");
        }

        tracing::debug!("Creating version symlinks");
        open_version_directory_for_activation(
            &versions_root,
            Path::new(&version_name),
            &version_dir,
            &pinned_version_root,
        )?;
        crate::fs::create_version_symlinks_in(
            &install_root,
            &pinned_version_root,
            &target_dir,
            &version_name,
        )?;
        shell_utils::setup_path_in(&install_root, &pinned_version_root, &target_dir)?;
        drop(pinned_version_root);

        println!(
            "Installed WasmEdge {version}\nInstall root: {}",
            target_dir.display()
        );

        Ok(())
    }
}

fn claim_version_directory(
    versions_root: &Dir,
    version_name: &Path,
    version_dir: &Path,
) -> Result<(Dir, Option<cap_tempfile::TempDir>)> {
    let metadata = match versions_root.symlink_metadata(version_name) {
        Ok(metadata) => Some(metadata),
        Err(error) if error.kind() == ErrorKind::NotFound => None,
        Err(error) => return Err(error.into()),
    };

    if let Some(metadata) = metadata {
        if metadata.file_type().is_symlink() || !metadata.is_dir() {
            return Err(Error::InvalidPath {
                path: version_dir.display().to_string(),
                reason: "the version path must be a real directory, not a symlink or file"
                    .to_string(),
            });
        }

        let version_root = crate::fs::open_cap_dir_nofollow(versions_root, version_name)?;
        if !valid_version_marker(&version_root)?
            && !crate::fs::cap_version_has_runtime(&version_root)?
        {
            return Err(Error::InvalidPath {
                path: version_dir.display().to_string(),
                reason: "refusing to claim an existing directory that is not owned by wasmedgeup"
                    .to_string(),
            });
        }
        write_version_marker(&version_root, version_dir)?;
        return Ok((version_root, None));
    }

    // Build a new version in a private sibling and publish the complete,
    // marked directory with one no-replace rename. A crash or copy failure
    // therefore leaves the final semantic-version path absent rather than an
    // unowned partial directory that future install/remove commands reject.
    let staging = cap_tempfile::tempdir_in(versions_root)?;
    write_version_staging_marker(&staging)?;
    staging.create_dir("entry")?;
    #[cfg(windows)]
    let version_root = crate::fs::open_cap_dir_nofollow_for_rename(&staging, Path::new("entry"))?;
    #[cfg(not(windows))]
    let version_root = crate::fs::open_cap_dir_nofollow(&staging, Path::new("entry"))?;
    write_version_marker(&version_root, version_dir)?;
    Ok((version_root, Some(staging)))
}

fn publish_version_directory(
    version_root: Dir,
    staging: cap_tempfile::TempDir,
    versions_root: &Dir,
    version_name: &Path,
    version_dir: &Path,
) -> Result<Dir> {
    #[cfg(windows)]
    let pinned_version_root =
        crate::fs::rename_noreplace_pinned_dir(version_root, versions_root, version_name).map_err(
            |error| Error::InvalidPath {
                path: version_dir.display().to_string(),
                reason: format!(
                    "the version path changed while the installation was being prepared: {error}"
                ),
            },
        )?;
    #[cfg(not(windows))]
    let pinned_version_root = {
        crate::fs::rename_noreplace(&staging, Path::new("entry"), versions_root, version_name)
            .map_err(|error| Error::InvalidPath {
                path: version_dir.display().to_string(),
                reason: format!(
                    "the version path changed while the installation was being prepared: {error}"
                ),
            })?;
        version_root
    };

    #[cfg(unix)]
    crate::fs::sync_cap_directory(versions_root)?;
    if let Err(error) = staging.close() {
        tracing::warn!(%error, "Failed to remove empty version staging directory");
    }
    Ok(pinned_version_root)
}

fn open_version_directory_for_activation(
    versions_root: &Dir,
    version_name: &Path,
    version_dir: &Path,
    pinned_version_root: &Dir,
) -> Result<()> {
    #[cfg(windows)]
    let activation_handle = same_file::Handle::from_file(
        crate::fs::open_cap_dir_identity_nofollow(versions_root, version_name).map_err(
            |error| Error::InvalidPath {
                path: version_dir.display().to_string(),
                reason: format!("the version path changed before activation: {error}"),
            },
        )?,
    )?;
    #[cfg(not(windows))]
    let activation_handle = {
        let activation_root = crate::fs::open_cap_dir_nofollow(versions_root, version_name)
            .map_err(|error| Error::InvalidPath {
                path: version_dir.display().to_string(),
                reason: format!("the version path changed before activation: {error}"),
            })?;
        same_file::Handle::from_file(activation_root.into_std_file())?
    };

    let pinned_handle =
        same_file::Handle::from_file(pinned_version_root.try_clone()?.into_std_file())?;
    if pinned_handle != activation_handle {
        return Err(Error::InvalidPath {
            path: version_dir.display().to_string(),
            reason: "the version directory was replaced before activation".to_string(),
        });
    }

    Ok(())
}

fn write_version_staging_marker(staging: &Dir) -> Result<()> {
    let mut options = OpenOptions::new();
    options
        .write(true)
        .create_new(true)
        ._cap_fs_ext_follow(FollowSymlinks::No);
    let mut file = staging.open_with(VERSION_STAGING_MARKER, &options)?;
    file.write_all(VERSION_STAGING_MARKER_CONTENT.as_bytes())?;
    file.sync_data()?;
    drop(file);

    #[cfg(unix)]
    crate::fs::sync_cap_directory(staging)?;
    Ok(())
}

fn valid_version_marker(version_dir: &Dir) -> std::io::Result<bool> {
    let metadata = match version_dir.symlink_metadata(VERSION_INSTALL_MARKER) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(error),
    };
    if metadata.file_type().is_symlink()
        || !metadata.is_file()
        || metadata.len() != VERSION_INSTALL_MARKER_CONTENT.len() as u64
    {
        return Ok(false);
    }

    let mut options = OpenOptions::new();
    options.read(true)._cap_fs_ext_follow(FollowSymlinks::No);
    let file = version_dir.open_with(VERSION_INSTALL_MARKER, &options)?;
    let metadata = file.metadata()?;
    if !metadata.is_file() || metadata.len() != VERSION_INSTALL_MARKER_CONTENT.len() as u64 {
        return Ok(false);
    }
    let mut content = Vec::with_capacity(VERSION_INSTALL_MARKER_CONTENT.len() + 1);
    file.take(VERSION_INSTALL_MARKER_CONTENT.len() as u64 + 1)
        .read_to_end(&mut content)?;
    Ok(content == VERSION_INSTALL_MARKER_CONTENT.as_bytes())
}

fn write_version_marker(version_dir: &Dir, version_dir_display: &Path) -> Result<()> {
    match version_dir.symlink_metadata(VERSION_INSTALL_MARKER) {
        Ok(_) if valid_version_marker(version_dir)? => return Ok(()),
        Ok(_) => {
            return Err(Error::InvalidPath {
                path: version_dir_display
                    .join(VERSION_INSTALL_MARKER)
                    .display()
                    .to_string(),
                reason: "the wasmedgeup ownership marker is not a regular marker file".to_string(),
            });
        }
        Err(error) if error.kind() == ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }

    let marker_staging = cap_tempfile::tempdir_in(version_dir)?;
    let mut options = OpenOptions::new();
    options
        .write(true)
        .create_new(true)
        ._cap_fs_ext_follow(FollowSymlinks::No);
    let mut file = marker_staging.open_with("marker", &options)?;
    file.write_all(VERSION_INSTALL_MARKER_CONTENT.as_bytes())?;
    file.sync_data()?;
    drop(file);

    match crate::fs::rename_noreplace(
        &marker_staging,
        Path::new("marker"),
        version_dir,
        Path::new(VERSION_INSTALL_MARKER),
    ) {
        Ok(()) => {}
        Err(error)
            if error.kind() == ErrorKind::AlreadyExists && valid_version_marker(version_dir)? => {}
        Err(error) if error.kind() == ErrorKind::AlreadyExists => {
            return Err(Error::InvalidPath {
                path: version_dir_display
                    .join(VERSION_INSTALL_MARKER)
                    .display()
                    .to_string(),
                reason: "the wasmedgeup ownership marker changed while it was being created"
                    .to_string(),
            });
        }
        Err(error) => return Err(error.into()),
    }

    #[cfg(unix)]
    crate::fs::sync_cap_directory(version_dir)?;
    if let Err(error) = marker_staging.close() {
        tracing::warn!(%error, "Failed to remove empty marker staging directory");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use cap_std::ambient_authority;

    fn open_versions(path: &Path) -> Dir {
        Dir::open_ambient_dir(path, ambient_authority()).unwrap()
    }

    #[test]
    fn new_version_is_marked_before_atomic_publication() {
        let parent = tempfile::tempdir().unwrap();
        let version_dir = parent.path().join("0.14.1");
        let versions_root = open_versions(parent.path());

        let (version_root, staging) =
            claim_version_directory(&versions_root, Path::new("0.14.1"), &version_dir).unwrap();

        assert!(
            !version_dir.exists(),
            "the final path must remain absent while the version is incomplete"
        );
        assert!(valid_version_marker(&version_root).unwrap());
        let staging = staging.expect("new version must be staged");
        let mut staging_marker = String::new();
        staging
            .open(VERSION_STAGING_MARKER)
            .unwrap()
            .read_to_string(&mut staging_marker)
            .unwrap();
        assert_eq!(staging_marker, VERSION_STAGING_MARKER_CONTENT);
        let pinned_version_root = publish_version_directory(
            version_root,
            staging,
            &versions_root,
            Path::new("0.14.1"),
            &version_dir,
        )
        .unwrap();
        open_version_directory_for_activation(
            &versions_root,
            Path::new("0.14.1"),
            &version_dir,
            &pinned_version_root,
        )
        .unwrap();

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
        let versions_root = open_versions(parent.path());

        let (version_root, staging) =
            claim_version_directory(&versions_root, Path::new("0.14.1"), &version_dir).unwrap();

        assert!(staging.is_none());
        assert!(valid_version_marker(&version_root).unwrap());
    }

    #[test]
    fn claim_preserves_an_existing_unowned_directory() {
        let parent = tempfile::tempdir().unwrap();
        let version_dir = parent.path().join("0.14.1");
        std::fs::create_dir(&version_dir).unwrap();
        std::fs::write(version_dir.join("foreign-data"), "preserve").unwrap();
        let versions_root = open_versions(parent.path());

        assert!(
            claim_version_directory(&versions_root, Path::new("0.14.1"), &version_dir,).is_err()
        );
        assert_eq!(
            std::fs::read_to_string(version_dir.join("foreign-data")).unwrap(),
            "preserve"
        );
        assert!(!version_dir.join(VERSION_INSTALL_MARKER).exists());
    }

    #[test]
    fn activation_accepts_the_claimed_version_directory() {
        let parent = tempfile::tempdir().unwrap();
        let version_dir = parent.path().join("0.14.1");
        std::fs::create_dir_all(version_dir.join("bin")).unwrap();
        std::fs::write(version_dir.join("bin/wasmedge"), "runtime").unwrap();
        let versions_root = open_versions(parent.path());
        let (version_root, staging) =
            claim_version_directory(&versions_root, Path::new("0.14.1"), &version_dir).unwrap();

        assert!(staging.is_none());
        open_version_directory_for_activation(
            &versions_root,
            Path::new("0.14.1"),
            &version_dir,
            &version_root,
        )
        .unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn activation_rejects_a_replaced_existing_version_directory() {
        let parent = tempfile::tempdir().unwrap();
        let version_dir = parent.path().join("0.14.1");
        std::fs::create_dir_all(version_dir.join("bin")).unwrap();
        std::fs::write(version_dir.join("bin/wasmedge"), "runtime").unwrap();
        let versions_root = open_versions(parent.path());
        let (version_root, staging) =
            claim_version_directory(&versions_root, Path::new("0.14.1"), &version_dir).unwrap();
        assert!(staging.is_none());

        std::fs::rename(&version_dir, parent.path().join("claimed-version")).unwrap();
        std::fs::create_dir(&version_dir).unwrap();
        std::fs::write(version_dir.join("attacker-runtime"), "replacement").unwrap();

        let result = open_version_directory_for_activation(
            &versions_root,
            Path::new("0.14.1"),
            &version_dir,
            &version_root,
        );

        assert!(matches!(result, Err(Error::InvalidPath { .. })));
        assert_eq!(
            std::fs::read_to_string(version_dir.join("attacker-runtime")).unwrap(),
            "replacement"
        );
    }

    #[cfg(unix)]
    #[test]
    fn claim_does_not_follow_a_legacy_bin_symlink() {
        let parent = tempfile::tempdir().unwrap();
        let version_dir = parent.path().join("0.14.1");
        std::fs::create_dir(&version_dir).unwrap();

        let outside = tempfile::tempdir().unwrap();
        std::fs::write(outside.path().join("wasmedge"), "foreign runtime").unwrap();
        std::os::unix::fs::symlink(outside.path(), version_dir.join("bin")).unwrap();
        let versions_root = open_versions(parent.path());

        let result = claim_version_directory(&versions_root, Path::new("0.14.1"), &version_dir);

        assert!(matches!(result, Err(Error::InvalidPath { .. })));
        assert!(!version_dir.join(VERSION_INSTALL_MARKER).exists());
        assert_eq!(
            std::fs::read_to_string(outside.path().join("wasmedge")).unwrap(),
            "foreign runtime"
        );
    }
}

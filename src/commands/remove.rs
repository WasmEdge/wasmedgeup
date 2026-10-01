use std::{
    io::ErrorKind,
    path::{Path, PathBuf},
};

use clap::Parser;
use tokio::fs;

use crate::{
    api::latest_installed_version,
    cli::{CommandContext, CommandExecutor},
    commands::{resolve_install_path, use_cmd::UseArgs},
    prelude::*,
    shell_utils::uninstall_path,
};

const MANAGED_ROOT_LINKS: [&str; 4] = ["bin", "include", "lib", "plugin"];

#[derive(Debug, Parser)]
pub struct RemoveArgs {
    /// WasmEdge version to remove, e.g. `0.13.0`, `0.15.0`, etc.
    #[arg(default_value = "")]
    pub version: String,

    /// Remove all installed versions
    #[arg(long)]
    pub all: bool,

    /// Set the install location for the WasmEdge runtime
    ///
    /// Defaults to `$HOME/.wasmedge` on Unix-like systems and `%HOME%\.wasmedge` on Windows.
    #[arg(short, long)]
    pub path: Option<PathBuf>,
}

impl CommandExecutor for RemoveArgs {
    async fn execute(self, ctx: CommandContext) -> Result<()> {
        let target_dir = resolve_install_path(self.path)?;
        let versions_dir = target_dir.join("versions");

        let versions_metadata = match fs::symlink_metadata(&versions_dir).await {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == ErrorKind::NotFound => {
                if self.all {
                    return Err(Error::InvalidPath {
                        path: versions_dir.display().to_string(),
                        reason: "no WasmEdge installation found".to_string(),
                    });
                }
                return Err(Error::VersionNotFound {
                    version: self.version,
                });
            }
            Err(error) => return Err(error.into()),
        };

        if versions_metadata.file_type().is_symlink() || !versions_metadata.is_dir() {
            return Err(Error::InvalidPath {
                path: versions_dir.display().to_string(),
                reason: "the versions path must be a real directory, not a symlink or file"
                    .to_string(),
            });
        }

        let canonical_target_dir = fs::canonicalize(&target_dir).await?;
        let canonical_versions_dir = fs::canonicalize(&versions_dir).await?;
        if canonical_versions_dir.parent() != Some(canonical_target_dir.as_path()) {
            return Err(Error::InvalidPath {
                path: versions_dir.display().to_string(),
                reason: "the versions directory resolves outside the install root".to_string(),
            });
        }

        if !self.all && self.version.is_empty() {
            return Err(Error::InvalidPath {
                path: "version".to_string(),
                reason: "no version specified; provide a version or use --all".to_string(),
            });
        }

        if self.all {
            tracing::debug!("Removing all installed versions");
            remove_all_versions(&canonical_versions_dir).await?;
            if let Err(e) = uninstall_path(&target_dir) {
                tracing::warn!(error = %e.to_string(), "Failed to update shell rc files during --all removal");
            }
            cleanup_install_root(&target_dir, &canonical_target_dir).await?;
            tracing::info!("All versions and configuration removed successfully");
            return Ok(());
        }

        let bin_path = target_dir.join("bin");
        let current_version = if fs::symlink_metadata(&bin_path)
            .await
            .is_ok_and(|metadata| metadata.file_type().is_symlink())
        {
            let bin_link = fs::read_link(&bin_path).await?;
            tracing::debug!(link = ?bin_link, "Raw symlink path");

            let normalized = if bin_link.is_absolute() {
                bin_link
                    .strip_prefix(&target_dir)
                    .map(|p| p.to_path_buf())
                    .unwrap_or(bin_link.clone())
            } else {
                bin_link.clone()
            };

            let mut comps = normalized.components().peekable();
            let mut found: Option<String> = None;
            while let Some(comp) = comps.next() {
                if let std::path::Component::Normal(name) = comp {
                    if name == "versions" {
                        if let Some(std::path::Component::Normal(ver)) = comps.peek().copied() {
                            let v = ver.to_string_lossy().to_string();
                            tracing::debug!(version = %v, "Extracted version from symlink");
                            found = Some(v);
                        }
                        break;
                    }
                }
            }

            if found.is_none() {
                tracing::debug!(normalized = %normalized.display(), "Could not find versions/<ver> in symlink path");
            }
            found
        } else {
            tracing::debug!("No bin symlink found");
            None
        };

        let version = ctx
            .client
            .resolve_version(&self.version)
            .await
            .inspect_err(
                |e| tracing::error!(error = %e.to_string(), "Failed to resolve version"),
            )?;
        tracing::debug!(%version, "Resolved version for use");

        let version_dir = canonical_versions_dir.join(version.to_string());
        match fs::symlink_metadata(&version_dir).await {
            Ok(metadata) if metadata.is_dir() => {
                fs::remove_dir_all(&version_dir).await?;
                tracing::info!(version = %version, "Version removed successfully");
            }
            Ok(_) => {
                return Err(Error::InvalidPath {
                    path: version_dir.display().to_string(),
                    reason: "the version path must be a real directory, not a symlink or file"
                        .to_string(),
                });
            }
            Err(error) if error.kind() == ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }

        let removed_current = Some(version.to_string()) == current_version;

        let mut remaining_versions = 0;
        let mut dir_stream = fs::read_dir(&canonical_versions_dir).await?;
        while let Some(entry) = dir_stream.next_entry().await? {
            if entry.file_type().await?.is_dir() {
                remaining_versions += 1;
            }
        }

        if remaining_versions == 0 {
            tracing::debug!("No versions remaining, cleaning up configuration");
            if let Err(e) = uninstall_path(&target_dir) {
                tracing::warn!(error = %e.to_string(), "Failed to update shell rc files when cleaning up last version");
            }
            remove_dir_if_empty(&canonical_versions_dir).await?;
            cleanup_install_root(&target_dir, &canonical_target_dir).await?;
            tracing::info!("All versions and configuration removed successfully");
            return Ok(());
        }

        if removed_current && remaining_versions > 0 {
            tracing::debug!(removed_version = ?current_version, "Current version was removed");

            let latest_version = latest_installed_version(&canonical_versions_dir)?;

            if let Some(version) = latest_version {
                tracing::info!(version = %version, "Switching to latest version");
                let use_args = UseArgs {
                    version: version.to_string(),
                    path: Some(target_dir),
                };
                use_args.execute(ctx).await?;
            } else {
                tracing::warn!("No other versions found to switch to");
            }
        }

        Ok(())
    }
}

async fn remove_all_versions(versions_dir: &Path) -> Result<()> {
    let mut version_dirs = Vec::new();
    let mut entries = fs::read_dir(versions_dir).await?;

    while let Some(entry) = entries.next_entry().await? {
        let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
            continue;
        };
        if semver::Version::parse(&name).is_err() {
            continue;
        }

        let path = entry.path();
        if !entry.file_type().await?.is_dir() {
            return Err(Error::InvalidPath {
                path: path.display().to_string(),
                reason: "the version path must be a real directory, not a symlink or file"
                    .to_string(),
            });
        }

        let canonical_path = fs::canonicalize(&path).await?;
        if canonical_path.parent() != Some(versions_dir) {
            return Err(Error::InvalidPath {
                path: path.display().to_string(),
                reason: "the version directory resolves outside the versions directory".to_string(),
            });
        }
        version_dirs.push(canonical_path);
    }

    for version_dir in version_dirs {
        fs::remove_dir_all(version_dir).await?;
    }
    remove_dir_if_empty(versions_dir).await?;

    Ok(())
}

async fn cleanup_install_root(target_dir: &Path, canonical_target_dir: &Path) -> Result<()> {
    for name in MANAGED_ROOT_LINKS {
        let path = target_dir.join(name);
        match fs::symlink_metadata(&path).await {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                let link_target = fs::read_link(&path).await?;
                if is_managed_root_link(target_dir, canonical_target_dir, name, &link_target) {
                    remove_symlink(&path).await?;
                } else {
                    tracing::debug!(path = %path.display(), "Preserving unmanaged install-root symlink");
                }
            }
            Ok(_) => {
                tracing::debug!(path = %path.display(), "Preserving unmanaged install-root entry");
            }
            Err(error) if error.kind() == ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
    }

    let target_metadata = match fs::symlink_metadata(target_dir).await {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error.into()),
    };

    if target_metadata.file_type().is_symlink() {
        if remove_dir_if_empty(canonical_target_dir).await? {
            remove_symlink(target_dir).await?;
        }
    } else {
        remove_dir_if_empty(target_dir).await?;
    }

    Ok(())
}

fn is_managed_root_link(
    target_dir: &Path,
    canonical_target_dir: &Path,
    name: &str,
    link_target: &Path,
) -> bool {
    let relative_target = if link_target.is_absolute() {
        match link_target
            .strip_prefix(target_dir)
            .or_else(|_| link_target.strip_prefix(canonical_target_dir))
        {
            Ok(path) => path,
            Err(_) => return false,
        }
    } else {
        link_target
    };

    let mut components = relative_target.components();
    let (
        Some(std::path::Component::Normal(versions)),
        Some(std::path::Component::Normal(version)),
        Some(std::path::Component::Normal(link_name)),
        None,
    ) = (
        components.next(),
        components.next(),
        components.next(),
        components.next(),
    )
    else {
        return false;
    };

    versions == std::ffi::OsStr::new("versions")
        && link_name == std::ffi::OsStr::new(name)
        && version
            .to_str()
            .is_some_and(|version| semver::Version::parse(version).is_ok())
}

async fn remove_dir_if_empty(path: &Path) -> Result<bool> {
    match fs::remove_dir(path).await {
        Ok(()) => Ok(true),
        Err(error)
            if matches!(
                error.kind(),
                ErrorKind::DirectoryNotEmpty | ErrorKind::NotFound
            ) =>
        {
            Ok(false)
        }
        Err(error) => Err(error.into()),
    }
}

#[cfg(unix)]
async fn remove_symlink(path: &Path) -> Result<()> {
    fs::remove_file(path).await?;
    Ok(())
}

#[cfg(windows)]
async fn remove_symlink(path: &Path) -> Result<()> {
    if fs::remove_dir(path).await.is_err() {
        fs::remove_file(path).await?;
    }
    Ok(())
}

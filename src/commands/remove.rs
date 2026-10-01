use std::{
    io::ErrorKind,
    path::{Path, PathBuf},
};

use cap_std::{ambient_authority, fs::Dir};
use clap::Parser;
use semver::Version;
use tokio::fs;

use crate::{
    cli::{CommandContext, CommandExecutor},
    commands::resolve_install_path,
    error::join_err_to_io_error,
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

        let target_metadata = match fs::symlink_metadata(&target_dir).await {
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

        if target_metadata.file_type().is_symlink() || !target_metadata.is_dir() {
            return Err(Error::InvalidPath {
                path: target_dir.display().to_string(),
                reason: "the install root must be a real directory, not a symlink or file"
                    .to_string(),
            });
        }

        let canonical_target_dir = fs::canonicalize(&target_dir).await?;
        let install_root = Dir::open_ambient_dir(&canonical_target_dir, ambient_authority())?;
        let versions_metadata = match install_root.symlink_metadata("versions") {
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

        let versions_root = install_root.open_dir("versions")?;

        if !self.all && self.version.is_empty() {
            return Err(Error::InvalidPath {
                path: "version".to_string(),
                reason: "no version specified; provide a version or use --all".to_string(),
            });
        }

        if self.all {
            tracing::debug!("Removing all installed versions");
            let managed_versions = managed_versions(&versions_root)?;
            remove_all_versions(&versions_root, &managed_versions).await?;
            if let Err(e) = uninstall_path(&target_dir) {
                tracing::warn!(error = %e.to_string(), "Failed to update shell rc files during --all removal");
            }
            drop(versions_root);
            remove_cap_dir_if_empty(&install_root, Path::new("versions"))?;
            cleanup_managed_root_entries(
                &install_root,
                &target_dir,
                &canonical_target_dir,
                &managed_versions,
            )?;
            drop(install_root);
            remove_path_dir_if_empty(&target_dir).await?;
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

        let version_name = version.to_string();
        let version_path = versions_dir.join(&version_name);
        let managed_versions_before = managed_versions(&versions_root)?;
        match versions_root.symlink_metadata(&version_name) {
            Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => {
                if !is_managed_version_dir(&versions_root, Path::new(&version_name))? {
                    return Err(Error::InvalidPath {
                        path: version_path.display().to_string(),
                        reason: "the version directory does not contain a WasmEdge runtime"
                            .to_string(),
                    });
                }
                remove_version_dir(&versions_root, version_name.clone()).await?;
                tracing::info!(version = %version, "Version removed successfully");
            }
            Ok(_) => {
                return Err(Error::InvalidPath {
                    path: version_path.display().to_string(),
                    reason: "the version path must be a real directory, not a symlink or file"
                        .to_string(),
                });
            }
            Err(error) if error.kind() == ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }

        let removed_current = Some(version.to_string()) == current_version;
        let latest_version = latest_managed_version(&versions_root)?;

        if latest_version.is_none() {
            tracing::debug!("No versions remaining, cleaning up configuration");
            if let Err(e) = uninstall_path(&target_dir) {
                tracing::warn!(error = %e.to_string(), "Failed to update shell rc files when cleaning up last version");
            }
            drop(versions_root);
            remove_cap_dir_if_empty(&install_root, Path::new("versions"))?;
            cleanup_managed_root_entries(
                &install_root,
                &target_dir,
                &canonical_target_dir,
                &managed_versions_before,
            )?;
            drop(install_root);
            remove_path_dir_if_empty(&target_dir).await?;
            tracing::info!("All versions and configuration removed successfully");
            return Ok(());
        }

        if removed_current {
            tracing::debug!(removed_version = ?current_version, "Current version was removed");

            if let Some(version) = latest_version {
                tracing::info!(version = %version, "Switching to latest version");
                retarget_managed_root_links(
                    &install_root,
                    &target_dir,
                    &canonical_target_dir,
                    &version.to_string(),
                    &managed_versions_before,
                )?;
                println!("Switched to WasmEdge runtime version: {version}");
            }
        }

        Ok(())
    }
}

async fn remove_all_versions(versions_dir: &Dir, managed_versions: &[Version]) -> Result<()> {
    for version in managed_versions {
        remove_version_dir(versions_dir, version.to_string()).await?;
    }
    Ok(())
}

fn managed_versions(versions_dir: &Dir) -> Result<Vec<Version>> {
    let mut versions = Vec::new();

    for entry in versions_dir.entries()? {
        let entry = entry?;
        let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
            continue;
        };
        let Ok(version) = Version::parse(&name) else {
            continue;
        };

        if entry.file_type()?.is_dir() && is_managed_version_dir(versions_dir, Path::new(&name))? {
            versions.push(version);
        } else {
            tracing::debug!(version = %name, "Preserving unowned semantic-version entry");
        }
    }

    Ok(versions)
}

fn latest_managed_version(versions_dir: &Dir) -> Result<Option<Version>> {
    Ok(managed_versions(versions_dir)?.into_iter().max())
}

fn is_managed_version_dir(versions_dir: &Dir, name: &Path) -> Result<bool> {
    let metadata = match versions_dir.symlink_metadata(name) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(error.into()),
    };
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Ok(false);
    }

    let version_dir = versions_dir.open_dir(name)?;
    for binary in ["bin/wasmedge", "bin/wasmedge.exe"] {
        match version_dir.symlink_metadata(binary) {
            Ok(metadata) if metadata.is_file() && !metadata.file_type().is_symlink() => {
                return Ok(true);
            }
            Ok(_) => {}
            Err(error) if error.kind() == ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
    }

    Ok(false)
}

async fn remove_version_dir(versions_dir: &Dir, name: String) -> Result<()> {
    let versions_dir = versions_dir.try_clone()?;
    tokio::task::spawn_blocking(move || versions_dir.remove_dir_all(name))
        .await
        .map_err(join_err_to_io_error)??;
    Ok(())
}

fn retarget_managed_root_links(
    install_root: &Dir,
    target_dir: &Path,
    canonical_target_dir: &Path,
    version: &str,
    managed_versions: &[Version],
) -> Result<()> {
    for name in MANAGED_ROOT_LINKS {
        let display_path = target_dir.join(name);
        let replace = match install_root.symlink_metadata(name) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                let link_target = install_root.read_link_contents(name)?;
                is_managed_root_link(
                    target_dir,
                    canonical_target_dir,
                    name,
                    &link_target,
                    managed_versions,
                )
            }
            Ok(_) => false,
            Err(error) if error.kind() == ErrorKind::NotFound => true,
            Err(error) => return Err(error.into()),
        };

        if !replace {
            tracing::debug!(path = %display_path.display(), "Preserving unmanaged install-root entry");
            continue;
        }

        if install_root.symlink_metadata(name).is_ok() {
            remove_symlink(install_root, name)?;
        }
        create_managed_root_link(install_root, target_dir, version, name)?;
    }

    Ok(())
}

fn cleanup_managed_root_entries(
    install_root: &Dir,
    target_dir: &Path,
    canonical_target_dir: &Path,
    managed_versions: &[Version],
) -> Result<()> {
    for name in MANAGED_ROOT_LINKS {
        let path = target_dir.join(name);
        match install_root.symlink_metadata(name) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                let link_target = install_root.read_link_contents(name)?;
                if is_managed_root_link(
                    target_dir,
                    canonical_target_dir,
                    name,
                    &link_target,
                    managed_versions,
                ) {
                    remove_symlink(install_root, name)?;
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
    Ok(())
}

fn is_managed_root_link(
    target_dir: &Path,
    canonical_target_dir: &Path,
    name: &str,
    link_target: &Path,
    managed_versions: &[Version],
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

    let Some(version) = version
        .to_str()
        .and_then(|value| Version::parse(value).ok())
    else {
        return false;
    };

    versions == std::ffi::OsStr::new("versions")
        && link_name == std::ffi::OsStr::new(name)
        && managed_versions.contains(&version)
}

fn remove_cap_dir_if_empty(parent: &Dir, path: &Path) -> Result<bool> {
    match parent.remove_dir(path) {
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

async fn remove_path_dir_if_empty(path: &Path) -> Result<bool> {
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
fn remove_symlink(install_root: &Dir, name: &str) -> Result<()> {
    install_root.remove_file(name)?;
    Ok(())
}

#[cfg(windows)]
fn remove_symlink(install_root: &Dir, name: &str) -> Result<()> {
    if install_root.remove_dir(name).is_err() {
        install_root.remove_file(name)?;
    }
    Ok(())
}

#[cfg(unix)]
fn create_managed_root_link(
    install_root: &Dir,
    _target_dir: &Path,
    version: &str,
    name: &str,
) -> Result<()> {
    let target = Path::new("versions").join(version).join(name);
    install_root.symlink(target, name)?;
    Ok(())
}

#[cfg(windows)]
fn create_managed_root_link(
    install_root: &Dir,
    target_dir: &Path,
    version: &str,
    name: &str,
) -> Result<()> {
    let target = target_dir.join("versions").join(version).join(name);
    install_root.symlink_dir(target, name)?;
    Ok(())
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::fs::symlink;

    #[tokio::test]
    async fn pinned_versions_handle_does_not_follow_replaced_parent() {
        let install = tempfile::tempdir().unwrap();
        let original_version = install.path().join("versions/0.14.1");
        std::fs::create_dir_all(&original_version).unwrap();
        std::fs::write(original_version.join("managed"), "runtime").unwrap();

        let outside = tempfile::tempdir().unwrap();
        let outside_version = outside.path().join("0.14.1");
        std::fs::create_dir_all(&outside_version).unwrap();
        let sentinel = outside_version.join("do-not-delete");
        std::fs::write(&sentinel, "outside data").unwrap();

        let install_root = Dir::open_ambient_dir(install.path(), ambient_authority()).unwrap();
        let versions_root = install_root.open_dir("versions").unwrap();
        std::fs::rename(
            install.path().join("versions"),
            install.path().join("original-versions"),
        )
        .unwrap();
        symlink(outside.path(), install.path().join("versions")).unwrap();

        remove_version_dir(&versions_root, "0.14.1".to_string())
            .await
            .unwrap();

        assert!(sentinel.exists(), "replacement symlink target must remain");
        assert!(
            !install.path().join("original-versions/0.14.1").exists(),
            "deletion must stay anchored to the opened versions directory"
        );
    }
}

use std::{
    io::{self, ErrorKind},
    path::{Path, PathBuf},
};

use cap_std::{ambient_authority, fs::Dir};
use clap::Parser;
use semver::Version;

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
        let target_dir = std::path::absolute(resolve_install_path(self.path)?)?;
        let versions_dir = target_dir.join("versions");

        let install_root = match open_install_root(&target_dir) {
            Ok(install_root) => install_root,
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

        let versions_root = match open_cap_dir_nofollow(&install_root, Path::new("versions")) {
            Ok(versions_root) => versions_root,
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
            Err(error) => {
                return Err(Error::InvalidPath {
                    path: versions_dir.display().to_string(),
                    reason: format!(
                        "the versions path must be a real directory opened without following \
                         symlinks: {error}"
                    ),
                });
            }
        };

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
            remove_open_dir_if_empty(versions_root)?;
            cleanup_managed_root_entries(&install_root, &target_dir, &managed_versions)?;
            remove_open_dir_if_empty(install_root)?;
            tracing::info!("All versions and configuration removed successfully");
            return Ok(());
        }

        let managed_versions_before = managed_versions(&versions_root)?;
        let current_version = root_link_version(&install_root, &target_dir, "bin")?
            .filter(|version| managed_versions_before.contains(version));

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
        let removed = match versions_root.symlink_metadata(&version_name) {
            Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => {
                let Some(version_dir) =
                    open_managed_version_dir(&versions_root, Path::new(&version_name))?
                else {
                    return Err(Error::InvalidPath {
                        path: version_path.display().to_string(),
                        reason: "the version directory does not contain a WasmEdge runtime"
                            .to_string(),
                    });
                };
                remove_version_dir(version_dir).await?;
                tracing::info!(version = %version, "Version removed successfully");
                true
            }
            Ok(_) => {
                return Err(Error::InvalidPath {
                    path: version_path.display().to_string(),
                    reason: "the version path must be a real directory, not a symlink or file"
                        .to_string(),
                });
            }
            Err(error) if error.kind() == ErrorKind::NotFound => false,
            Err(error) => return Err(error.into()),
        };

        let removed_current = current_version.as_ref() == Some(&version);
        let latest_version = latest_managed_version(&versions_root)?;

        if latest_version.is_none() {
            tracing::debug!("No versions remaining, cleaning up configuration");
            if let Err(e) = uninstall_path(&target_dir) {
                tracing::warn!(error = %e.to_string(), "Failed to update shell rc files when cleaning up last version");
            }
            remove_open_dir_if_empty(versions_root)?;
            cleanup_managed_root_entries(&install_root, &target_dir, &managed_versions_before)?;
            remove_open_dir_if_empty(install_root)?;
            tracing::info!("All versions and configuration removed successfully");
            return Ok(());
        }

        if removed {
            let latest_version = latest_version.expect("checked above");
            retarget_removed_version_links(
                &install_root,
                &target_dir,
                &version,
                &latest_version,
                &managed_versions_before,
                removed_current,
            )?;

            if removed_current {
                tracing::info!(version = %latest_version, "Switching to latest version");
                println!("Switched to WasmEdge runtime version: {latest_version}");
            }
        }

        Ok(())
    }
}

fn open_install_root(path: &Path) -> io::Result<Dir> {
    let parent_path = path.parent().ok_or_else(|| {
        io::Error::new(
            ErrorKind::InvalidInput,
            "the install root must have a parent directory",
        )
    })?;
    let name = path.file_name().ok_or_else(|| {
        io::Error::new(
            ErrorKind::InvalidInput,
            "the install root must not be a filesystem root",
        )
    })?;
    let parent = Dir::open_ambient_dir(parent_path, ambient_authority())?;
    open_cap_dir_nofollow(&parent, Path::new(name))
}

fn open_cap_dir_nofollow(parent: &Dir, path: &Path) -> io::Result<Dir> {
    let parent_file = parent.try_clone()?.into_std_file();
    let dir = cap_primitives::fs::open_dir_nofollow(&parent_file, path)?;
    Ok(Dir::from_std_file(dir))
}

async fn remove_all_versions(versions_dir: &Dir, managed_versions: &[Version]) -> Result<()> {
    for version in managed_versions {
        let name = version.to_string();
        if let Some(version_dir) = open_managed_version_dir(versions_dir, Path::new(&name))? {
            remove_version_dir(version_dir).await?;
        } else {
            tracing::debug!(%version, "Preserving version entry that is no longer managed");
        }
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

        if entry.file_type()?.is_dir()
            && open_managed_version_dir(versions_dir, Path::new(&name))?.is_some()
        {
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

fn open_managed_version_dir(versions_dir: &Dir, name: &Path) -> Result<Option<Dir>> {
    let metadata = match versions_dir.symlink_metadata(name) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Ok(None);
    }

    let version_dir = open_cap_dir_nofollow(versions_dir, name)?;
    for binary in ["bin/wasmedge", "bin/wasmedge.exe"] {
        match version_dir.symlink_metadata(binary) {
            Ok(metadata) if metadata.is_file() && !metadata.file_type().is_symlink() => {
                return Ok(Some(version_dir));
            }
            Ok(_) => {}
            Err(error) if error.kind() == ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
    }

    Ok(None)
}

async fn remove_version_dir(version_dir: Dir) -> Result<()> {
    tokio::task::spawn_blocking(move || version_dir.remove_open_dir_all())
        .await
        .map_err(join_err_to_io_error)??;
    Ok(())
}

fn retarget_removed_version_links(
    install_root: &Dir,
    target_dir: &Path,
    removed_version: &Version,
    replacement_version: &Version,
    managed_versions: &[Version],
    create_missing: bool,
) -> Result<()> {
    for name in MANAGED_ROOT_LINKS {
        let display_path = target_dir.join(name);
        let replace = match install_root.symlink_metadata(name) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                let link_target = install_root.read_link_contents(name)?;
                root_link_target_version(target_dir, name, &link_target).is_some_and(|version| {
                    version == *removed_version && managed_versions.contains(&version)
                })
            }
            Ok(_) => false,
            Err(error) if error.kind() == ErrorKind::NotFound => create_missing,
            Err(error) => return Err(error.into()),
        };

        if !replace {
            tracing::debug!(path = %display_path.display(), "Preserving unmanaged install-root entry");
            continue;
        }

        if install_root.symlink_metadata(name).is_ok() {
            remove_symlink(install_root, name)?;
        }
        create_managed_root_link(
            install_root,
            target_dir,
            &replacement_version.to_string(),
            name,
        )?;
    }

    Ok(())
}

fn cleanup_managed_root_entries(
    install_root: &Dir,
    target_dir: &Path,
    managed_versions: &[Version],
) -> Result<()> {
    for name in MANAGED_ROOT_LINKS {
        let path = target_dir.join(name);
        match install_root.symlink_metadata(name) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                let link_target = install_root.read_link_contents(name)?;
                let managed = root_link_target_version(target_dir, name, &link_target)
                    .is_some_and(|version| managed_versions.contains(&version));
                if managed {
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

fn root_link_version(install_root: &Dir, target_dir: &Path, name: &str) -> Result<Option<Version>> {
    match install_root.symlink_metadata(name) {
        Ok(metadata) if metadata.file_type().is_symlink() => {
            let link_target = install_root.read_link_contents(name)?;
            Ok(root_link_target_version(target_dir, name, &link_target))
        }
        Ok(_) => Ok(None),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.into()),
    }
}

fn root_link_target_version(target_dir: &Path, name: &str, link_target: &Path) -> Option<Version> {
    let relative_target = if link_target.is_absolute() {
        link_target.strip_prefix(target_dir).ok()?
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
        return None;
    };

    if versions != std::ffi::OsStr::new("versions") || link_name != std::ffi::OsStr::new(name) {
        return None;
    }

    version
        .to_str()
        .and_then(|value| Version::parse(value).ok())
}

fn remove_open_dir_if_empty(dir: Dir) -> Result<bool> {
    match dir.remove_open_dir() {
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
    _target_dir: &Path,
    version: &str,
    name: &str,
) -> Result<()> {
    let target = Path::new("versions").join(version).join(name);
    install_root.symlink_dir(target, name)?;
    Ok(())
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::fs::symlink;

    #[test]
    fn install_root_open_does_not_follow_symlinks() {
        let parent = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let install_link = parent.path().join("install");
        symlink(outside.path(), &install_link).unwrap();

        assert!(
            open_install_root(&install_link).is_err(),
            "opening the install root must reject a symlink"
        );
    }

    #[tokio::test]
    async fn pinned_version_handle_does_not_follow_replaced_entry() {
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
        let version_dir = open_cap_dir_nofollow(&versions_root, Path::new("0.14.1")).unwrap();
        std::fs::rename(
            install.path().join("versions/0.14.1"),
            install.path().join("versions/original-0.14.1"),
        )
        .unwrap();
        symlink(&outside_version, install.path().join("versions/0.14.1")).unwrap();

        remove_version_dir(version_dir).await.unwrap();

        assert!(sentinel.exists(), "replacement symlink target must remain");
        assert!(
            !install.path().join("versions/original-0.14.1").exists(),
            "deletion must stay anchored to the verified version directory"
        );
    }
}

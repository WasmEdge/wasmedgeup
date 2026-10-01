use std::{
    io::ErrorKind,
    path::{Path, PathBuf},
};

#[cfg(windows)]
use cap_std::ambient_authority;
use cap_std::fs::Dir;
use clap::Parser;
use semver::Version;

use crate::{
    cli::{CommandContext, CommandExecutor},
    commands::{
        normalize_absolute_path, resolve_install_path,
        runtime::{
            cap_file_matches, latest_usable_managed_version, managed_version_is_usable,
            managed_versions, open_managed_version_dir,
        },
    },
    constants::{VERSION_STAGING_MARKER, VERSION_STAGING_MARKER_CONTENT},
    error::join_err_to_io_error,
    fs::{
        open_cap_dir_nofollow, open_dir_nofollow as open_install_root, quarantine_entry,
        restore_quarantined_entry,
    },
    prelude::*,
    shell_utils::{remove_managed_shell_scripts, uninstall_path_configuration},
};

#[cfg(all(test, unix))]
use crate::shell_utils::managed_shell_script_content;

const MANAGED_ROOT_LINKS: [&str; 4] = ["bin", "include", "lib", "plugin"];

#[derive(Debug, Clone)]
struct ManagedRootLink {
    name: &'static str,
    version: Version,
    target: PathBuf,
}

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
        let configured_target_dir = resolve_install_path(self.path)?;
        let target_dir = normalize_absolute_path(&configured_target_dir)?;
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
            let managed_links = managed_root_links(
                &install_root,
                &versions_root,
                &target_dir,
                &configured_target_dir,
                &managed_versions,
            )?;
            remove_all_versions(&versions_root, &managed_versions).await?;
            remove_managed_version_staging_directories(&versions_root).await?;
            let shell_cleanup = cleanup_shell_integration(
                &install_root,
                &target_dir,
                &configured_target_dir,
                "during --all removal",
            );
            finish_install_cleanup(
                shell_cleanup,
                install_root,
                versions_root,
                &target_dir,
                &managed_links,
            )?;
            tracing::info!("All versions and configuration removed successfully");
            return Ok(());
        }

        let managed_versions_before = managed_versions(&versions_root)?;
        let managed_links_before = managed_root_links(
            &install_root,
            &versions_root,
            &target_dir,
            &configured_target_dir,
            &managed_versions_before,
        )?;
        let current_version = managed_links_before
            .iter()
            .find(|link| link.name == "bin")
            .map(|link| link.version.clone());

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
        let latest_version = latest_usable_managed_version(&versions_root)?;

        if latest_version.is_none() {
            let partial_versions_remain = !managed_versions(&versions_root)?.is_empty();
            tracing::debug!("No usable versions remaining, cleaning up configuration");
            let shell_cleanup = cleanup_shell_integration(
                &install_root,
                &target_dir,
                &configured_target_dir,
                "when cleaning up last version",
            );
            finish_install_cleanup(
                shell_cleanup,
                install_root,
                versions_root,
                &target_dir,
                &managed_links_before,
            )?;
            if partial_versions_remain {
                tracing::info!(
                    "No usable versions remain; configuration removed and partial installs preserved"
                );
            } else {
                tracing::info!("All versions and configuration removed successfully");
            }
            return Ok(());
        }

        if removed {
            let latest_version = latest_version.expect("checked above");
            let active_version = match current_version.as_ref() {
                Some(current_version) if !removed_current => {
                    managed_version_is_usable(&versions_root, current_version)?
                        .then_some(current_version)
                }
                _ => None,
            };
            let switch_current_version = current_version.is_some() && active_version.is_none();
            let replacement_version = active_version.unwrap_or(&latest_version);
            retarget_removed_version_links(
                &install_root,
                &target_dir,
                &version,
                replacement_version,
                &managed_links_before,
                switch_current_version,
            )?;

            if switch_current_version {
                tracing::info!(version = %latest_version, "Switching to latest version");
                println!("Switched to WasmEdge runtime version: {latest_version}");
            }
        }

        Ok(())
    }
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

async fn remove_managed_version_staging_directories(versions_dir: &Dir) -> Result<()> {
    let mut staging_dirs = Vec::new();

    for entry in versions_dir.entries()? {
        let entry = entry?;
        let file_type = entry.file_type()?;
        if !file_type.is_dir() || file_type.is_symlink() {
            continue;
        }

        let name = PathBuf::from(entry.file_name());
        let staging_dir = match open_cap_dir_nofollow(versions_dir, &name) {
            Ok(staging_dir) => staging_dir,
            Err(error) if error.kind() == ErrorKind::NotFound => continue,
            Err(error) => return Err(error.into()),
        };
        if cap_file_matches(
            &staging_dir,
            VERSION_STAGING_MARKER,
            VERSION_STAGING_MARKER_CONTENT,
        )? {
            staging_dirs.push((name, staging_dir));
        }
    }

    for (name, staging_dir) in staging_dirs {
        remove_version_dir(staging_dir).await?;
        tracing::debug!(path = %name.display(), "Removed interrupted version staging directory");
    }
    Ok(())
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
    managed_links: &[ManagedRootLink],
    switch_current_version: bool,
) -> Result<()> {
    for name in MANAGED_ROOT_LINKS {
        let display_path = target_dir.join(name);
        let managed_link = managed_links.iter().find(|link| link.name == name);
        let replace = managed_link
            .is_some_and(|link| switch_current_version || link.version == *removed_version);

        if !replace && !(switch_current_version && managed_link.is_none()) {
            tracing::debug!(path = %display_path.display(), "Preserving unmanaged install-root entry");
            continue;
        }

        let removed = match managed_link {
            Some(link) if replace => quarantine_managed_root_link(install_root, link)?,
            _ => false,
        };
        if !removed {
            if switch_current_version
                && install_root
                    .symlink_metadata(name)
                    .is_err_and(|error| error.kind() == ErrorKind::NotFound)
            {
                create_managed_root_link(
                    install_root,
                    target_dir,
                    &replacement_version.to_string(),
                    name,
                )?;
            }
            continue;
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
    managed_links: &[ManagedRootLink],
) -> Result<()> {
    for name in MANAGED_ROOT_LINKS {
        let path = target_dir.join(name);
        if let Some(link) = managed_links.iter().find(|link| link.name == name) {
            quarantine_managed_root_link(install_root, link)?;
        } else if install_root.symlink_metadata(name).is_ok() {
            tracing::debug!(path = %path.display(), "Preserving unmanaged install-root entry");
        }
    }
    Ok(())
}

fn managed_root_links(
    install_root: &Dir,
    versions_root: &Dir,
    target_dir: &Path,
    configured_target_dir: &Path,
    managed_versions: &[Version],
) -> Result<Vec<ManagedRootLink>> {
    let mut links = Vec::new();

    for name in MANAGED_ROOT_LINKS {
        let metadata = match install_root.symlink_metadata(name) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == ErrorKind::NotFound => continue,
            Err(error) => return Err(error.into()),
        };
        if !metadata.file_type().is_symlink() {
            continue;
        }

        let target = install_root.read_link_contents(name)?;
        if let Some(version) = root_link_target_version(
            versions_root,
            target_dir,
            configured_target_dir,
            name,
            &target,
            managed_versions,
        )? {
            links.push(ManagedRootLink {
                name,
                version,
                target,
            });
        }
    }

    Ok(links)
}

fn root_link_target_version(
    versions_root: &Dir,
    target_dir: &Path,
    configured_target_dir: &Path,
    name: &str,
    link_target: &Path,
    managed_versions: &[Version],
) -> Result<Option<Version>> {
    if let Some(version) =
        lexical_root_link_target_version(target_dir, configured_target_dir, name, link_target)
    {
        return Ok(managed_versions.contains(&version).then_some(version));
    }

    #[cfg(windows)]
    {
        return windows_root_link_target_version(
            versions_root,
            name,
            link_target,
            managed_versions,
        );
    }

    #[cfg(not(windows))]
    {
        let _ = versions_root;
        Ok(None)
    }
}

fn lexical_root_link_target_version(
    target_dir: &Path,
    configured_target_dir: &Path,
    name: &str,
    link_target: &Path,
) -> Option<Version> {
    let relative_target = if link_target.is_absolute() {
        link_target.strip_prefix(target_dir).ok()?
    } else if parse_relative_root_link_target(name, link_target).is_some() {
        link_target
    } else {
        #[cfg(windows)]
        {
            link_target.strip_prefix(configured_target_dir).ok()?
        }
        #[cfg(not(windows))]
        {
            let _ = configured_target_dir;
            return None;
        }
    };

    parse_relative_root_link_target(name, relative_target)
}

fn parse_relative_root_link_target(name: &str, relative_target: &Path) -> Option<Version> {
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

#[cfg(windows)]
fn windows_root_link_target_version(
    versions_root: &Dir,
    name: &str,
    link_target: &Path,
    managed_versions: &[Version],
) -> Result<Option<Version>> {
    let Some(link_name) = link_target.file_name().and_then(|value| value.to_str()) else {
        return Ok(None);
    };
    if !link_target.is_absolute()
        || !is_local_windows_path(link_target)
        || !link_name.eq_ignore_ascii_case(name)
    {
        return Ok(None);
    }
    let Some(link_parent) = link_target.parent() else {
        return Ok(None);
    };
    let Ok(actual_parent) = Dir::open_ambient_dir(link_parent, ambient_authority()) else {
        return Ok(None);
    };
    let actual_handle = same_file::Handle::from_file(actual_parent.into_std_file())?;

    for version in managed_versions {
        let Ok(expected_parent) =
            open_cap_dir_nofollow(versions_root, Path::new(&version.to_string()))
        else {
            continue;
        };
        let expected_handle = same_file::Handle::from_file(expected_parent.into_std_file())?;
        if actual_handle == expected_handle {
            return Ok(Some(version.clone()));
        }
    }

    Ok(None)
}

#[cfg(windows)]
fn is_local_windows_path(path: &Path) -> bool {
    use std::path::{Component, Prefix};

    matches!(
        path.components().next(),
        Some(Component::Prefix(prefix))
            if matches!(prefix.kind(), Prefix::Disk(_) | Prefix::VerbatimDisk(_))
    )
}

fn quarantine_managed_root_link(
    install_root: &Dir,
    managed_link: &ManagedRootLink,
) -> Result<bool> {
    let Some(quarantine) = quarantine_entry(install_root, managed_link.name)? else {
        return Ok(false);
    };
    let still_managed = quarantine
        .symlink_metadata("entry")
        .is_ok_and(|metadata| metadata.file_type().is_symlink())
        && quarantine
            .read_link_contents("entry")
            .is_ok_and(|target| target == managed_link.target);

    if !still_managed {
        restore_quarantined_entry(quarantine, install_root, managed_link.name)?;
        return Ok(false);
    }

    remove_symlink(&quarantine, "entry")?;
    quarantine.close()?;
    Ok(true)
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

fn finish_install_cleanup(
    shell_cleanup: Result<()>,
    install_root: Dir,
    versions_root: Dir,
    target_dir: &Path,
    managed_links: &[ManagedRootLink],
) -> Result<()> {
    // These operations clean up independent entries. Run each one even when
    // an earlier step fails so a restored or unreadable env script cannot
    // leave managed root links pointing at versions that were already removed.
    let links_cleanup = cleanup_managed_root_entries(&install_root, target_dir, managed_links);
    finish_install_cleanup_after_links(shell_cleanup, links_cleanup, install_root, versions_root)
}

fn finish_install_cleanup_after_links(
    shell_cleanup: Result<()>,
    links_cleanup: Result<()>,
    install_root: Dir,
    versions_root: Dir,
) -> Result<()> {
    let versions_cleanup = if shell_cleanup.is_ok() && links_cleanup.is_ok() {
        remove_open_dir_if_empty(versions_root)
    } else {
        // Keep the versions directory as a retry marker. Without it, a later
        // remove invocation would reject the remaining managed shell script or
        // root link as an absent installation before it could retry cleanup.
        Ok(false)
    };
    let root_cleanup = remove_open_dir_if_empty(install_root);

    shell_cleanup?;
    links_cleanup?;
    versions_cleanup?;
    root_cleanup?;
    Ok(())
}

fn cleanup_shell_integration(
    install_root: &Dir,
    target_dir: &Path,
    configured_target_dir: &Path,
    context: &str,
) -> Result<()> {
    let mut configuration_paths = vec![configured_target_dir.to_path_buf()];
    if target_dir != configured_target_dir {
        configuration_paths.push(target_dir.to_path_buf());
    }
    let cleanup = remove_managed_shell_scripts(install_root, target_dir, configured_target_dir);
    for path in cleanup.configured_paths {
        if !configuration_paths.contains(&path) {
            configuration_paths.push(path);
        }
    }

    for path in configuration_paths {
        if let Err(error) = uninstall_path_configuration(&path) {
            tracing::warn!(
                error = %error.to_string(),
                path = %path.display(),
                "Failed to update shell configuration {context}"
            );
        }
    }

    cleanup.result
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
    use cap_std::ambient_authority;
    use std::os::unix::fs::symlink;

    fn successful_shell_script_cleanup(
        install_root: &Dir,
        target_dir: &Path,
        configured_target_dir: &Path,
    ) -> Vec<PathBuf> {
        let cleanup = remove_managed_shell_scripts(install_root, target_dir, configured_target_dir);
        cleanup.result.unwrap();
        cleanup.configured_paths
    }

    #[test]
    fn install_root_open_does_not_follow_symlinks() {
        let parent = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let parent_path = std::fs::canonicalize(parent.path()).unwrap();
        let outside_path = std::fs::canonicalize(outside.path()).unwrap();
        let install_link = parent_path.join("install");
        symlink(outside_path, &install_link).unwrap();

        assert!(
            open_install_root(&install_link).is_err(),
            "opening the install root must reject a symlink"
        );
    }

    #[test]
    fn install_root_open_does_not_follow_parent_symlinks() {
        let parent = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let parent_path = std::fs::canonicalize(parent.path()).unwrap();
        let outside_path = std::fs::canonicalize(outside.path()).unwrap();
        std::fs::create_dir(outside_path.join("install")).unwrap();
        let parent_link = parent_path.join("redirect");
        symlink(outside_path, &parent_link).unwrap();

        assert!(
            open_install_root(&parent_link.join("install")).is_err(),
            "opening the install root must reject symlinks in parent components"
        );
    }

    #[test]
    fn managed_version_probe_does_not_follow_a_symlinked_bin_directory() {
        let install = tempfile::tempdir().unwrap();
        let versions_path = install.path().join("versions");
        let version_path = versions_path.join("0.14.1");
        std::fs::create_dir_all(&version_path).unwrap();

        let outside = tempfile::tempdir().unwrap();
        std::fs::write(outside.path().join("wasmedge"), "foreign runtime").unwrap();
        symlink(outside.path(), version_path.join("bin")).unwrap();
        let versions_root = Dir::open_ambient_dir(&versions_path, ambient_authority()).unwrap();

        assert!(
            open_managed_version_dir(&versions_root, Path::new("0.14.1"))
                .unwrap()
                .is_none(),
            "a version with a symlinked bin directory must remain unowned"
        );
    }

    #[test]
    fn pinned_shell_script_cleanup_ignores_replaced_install_path() {
        let parent = tempfile::tempdir().unwrap();
        let parent_path = std::fs::canonicalize(parent.path()).unwrap();
        let install_path = parent_path.join("install");
        std::fs::create_dir(&install_path).unwrap();
        let script = managed_shell_script_content("env", &install_path).unwrap();
        std::fs::write(install_path.join("env"), script).unwrap();
        let install_root = open_install_root(&install_path).unwrap();

        let renamed_path = parent_path.join("original-install");
        std::fs::rename(&install_path, &renamed_path).unwrap();
        let outside = tempfile::tempdir().unwrap();
        let outside_path = std::fs::canonicalize(outside.path()).unwrap();
        let outside_script = outside_path.join("env");
        std::fs::write(&outside_script, "outside data").unwrap();
        symlink(outside_path, &install_path).unwrap();

        successful_shell_script_cleanup(&install_root, &install_path, &install_path);

        assert!(outside_script.exists(), "outside env script must remain");
        assert!(
            !renamed_path.join("env").exists(),
            "managed script must be removed through the pinned root"
        );
    }

    #[test]
    fn shell_script_cleanup_preserves_template_for_another_root() {
        let install = tempfile::tempdir().unwrap();
        let install_path = std::fs::canonicalize(install.path()).unwrap();
        let other = tempfile::tempdir().unwrap();
        let other_path = std::fs::canonicalize(other.path()).unwrap();
        let script = managed_shell_script_content("env", &other_path).unwrap();
        std::fs::write(install_path.join("env"), script).unwrap();
        let install_root = open_install_root(&install_path).unwrap();

        let configured_paths =
            successful_shell_script_cleanup(&install_root, &install_path, &install_path);

        assert!(configured_paths.is_empty());
        assert!(
            install_path.join("env").exists(),
            "an official-looking script for another root must be preserved"
        );
    }

    #[test]
    fn shell_script_cleanup_recovers_legacy_relative_path_without_resolving_it() {
        let parent = tempfile::tempdir().unwrap();
        let install_path = std::fs::canonicalize(parent.path())
            .unwrap()
            .join("legacy-root");
        std::fs::create_dir(&install_path).unwrap();
        let configured_path = Path::new("./legacy-root");
        let script = managed_shell_script_content("env", configured_path).unwrap();
        std::fs::write(install_path.join("env"), script).unwrap();
        let install_root = open_install_root(&install_path).unwrap();

        let configured_paths =
            successful_shell_script_cleanup(&install_root, &install_path, configured_path);

        assert_eq!(configured_paths, vec![configured_path.to_path_buf()]);
        assert!(!install_path.join("env").exists());
    }

    #[test]
    fn shell_script_cleanup_preserves_relative_script_without_matching_request_spelling() {
        let parent = tempfile::tempdir().unwrap();
        let install_path = std::fs::canonicalize(parent.path())
            .unwrap()
            .join("install");
        std::fs::create_dir(&install_path).unwrap();
        let script = managed_shell_script_content("env", Path::new("./install")).unwrap();
        std::fs::write(install_path.join("env"), script).unwrap();
        let install_root = open_install_root(&install_path).unwrap();

        let configured_paths =
            successful_shell_script_cleanup(&install_root, &install_path, &install_path);

        assert!(configured_paths.is_empty());
        assert!(
            install_path.join("env").exists(),
            "a relative script that could have been copied from another root must be preserved"
        );
    }

    #[test]
    fn shell_script_cleanup_preserves_legacy_relative_path_for_another_root() {
        let parent = tempfile::tempdir().unwrap();
        let install_path = std::fs::canonicalize(parent.path())
            .unwrap()
            .join("managed-root");
        std::fs::create_dir(&install_path).unwrap();
        let script = managed_shell_script_content("env", Path::new("./other-root")).unwrap();
        std::fs::write(install_path.join("env"), script).unwrap();
        let install_root = open_install_root(&install_path).unwrap();

        let configured_paths =
            successful_shell_script_cleanup(&install_root, &install_path, &install_path);

        assert!(configured_paths.is_empty());
        assert!(install_path.join("env").exists());
    }

    #[test]
    fn quarantine_restores_an_unmanaged_replacement() {
        let install = tempfile::tempdir().unwrap();
        let install_path = std::fs::canonicalize(install.path()).unwrap();
        std::fs::write(install_path.join("bin"), "foreign data").unwrap();
        let install_root = open_install_root(&install_path).unwrap();
        let version = Version::parse("0.14.1").unwrap();
        let managed_link = ManagedRootLink {
            name: "bin",
            version,
            target: PathBuf::from("versions/0.14.1/bin"),
        };

        let removed = quarantine_managed_root_link(&install_root, &managed_link).unwrap();

        assert!(!removed, "an unmanaged replacement must not be deleted");
        assert_eq!(
            std::fs::read_to_string(install_path.join("bin")).unwrap(),
            "foreign data"
        );
    }

    #[test]
    fn shell_cleanup_error_preserves_retry_marker_after_cleaning_root_links() {
        let parent = tempfile::tempdir().unwrap();
        let install_path = parent.path().join("install");
        std::fs::create_dir_all(install_path.join("versions")).unwrap();
        symlink("versions/0.14.1/bin", install_path.join("bin")).unwrap();

        let install_root = open_install_root(&install_path).unwrap();
        let versions_root = open_cap_dir_nofollow(&install_root, Path::new("versions")).unwrap();
        let managed_links = vec![ManagedRootLink {
            name: "bin",
            version: Version::parse("0.14.1").unwrap(),
            target: PathBuf::from("versions/0.14.1/bin"),
        }];
        let shell_cleanup: Result<()> = Err(std::io::Error::other("simulated read failure").into());

        let error = finish_install_cleanup(
            shell_cleanup,
            install_root,
            versions_root,
            &install_path,
            &managed_links,
        )
        .unwrap_err();

        assert!(error.to_string().contains("simulated read failure"));
        assert!(
            install_path.join("versions").is_dir(),
            "the versions directory must remain so cleanup can be retried"
        );
        assert!(
            std::fs::symlink_metadata(install_path.join("bin")).is_err(),
            "independent root-link cleanup must finish before reporting the shell error"
        );

        let install_root = open_install_root(&install_path).unwrap();
        let versions_root = open_cap_dir_nofollow(&install_root, Path::new("versions")).unwrap();
        finish_install_cleanup(Ok(()), install_root, versions_root, &install_path, &[]).unwrap();

        assert!(
            !install_path.exists(),
            "a successful retry must finish removing the empty install root"
        );
    }

    #[test]
    fn root_link_cleanup_error_preserves_retry_marker() {
        let parent = tempfile::tempdir().unwrap();
        let install_path = parent.path().join("install");
        std::fs::create_dir_all(install_path.join("versions")).unwrap();
        let install_root = open_install_root(&install_path).unwrap();
        let versions_root = open_cap_dir_nofollow(&install_root, Path::new("versions")).unwrap();
        let links_cleanup: Result<()> =
            Err(std::io::Error::other("simulated root-link failure").into());

        let error =
            finish_install_cleanup_after_links(Ok(()), links_cleanup, install_root, versions_root)
                .unwrap_err();

        assert!(error.to_string().contains("simulated root-link failure"));
        assert!(
            install_path.join("versions").is_dir(),
            "the versions directory must remain so root-link cleanup can be retried"
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

#[cfg(all(test, windows))]
mod windows_tests {
    use super::{is_local_windows_path, lexical_root_link_target_version};
    use std::path::Path;

    #[test]
    fn windows_link_probe_accepts_only_local_drive_paths() {
        assert!(is_local_windows_path(Path::new(r"C:\WasmEdge\bin")));
        assert!(is_local_windows_path(Path::new(r"\\?\C:\WasmEdge\bin")));
        assert!(!is_local_windows_path(Path::new(r"\\server\share\bin")));
        assert!(!is_local_windows_path(Path::new(
            r"\\?\UNC\server\share\bin"
        )));
        assert!(!is_local_windows_path(Path::new(r"\\.\device\bin")));
    }

    #[test]
    fn recognizes_legacy_relative_custom_root_links() {
        let version = lexical_root_link_target_version(
            Path::new(r"C:\work\relative-root"),
            Path::new("relative-root"),
            "bin",
            Path::new(r"relative-root\versions\0.14.1\bin"),
        );

        assert_eq!(version, Some(semver::Version::parse("0.14.1").unwrap()));
        assert_eq!(
            lexical_root_link_target_version(
                Path::new(r"C:\work\relative-root"),
                Path::new("relative-root"),
                "bin",
                Path::new(r"other-root\versions\0.14.1\bin"),
            ),
            None
        );
    }
}

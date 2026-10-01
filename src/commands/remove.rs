use std::{
    io::{self, ErrorKind, Read},
    path::{Path, PathBuf},
};

use cap_std::{ambient_authority, fs::Dir};
use clap::Parser;
use semver::Version;

use crate::{
    cli::{CommandContext, CommandExecutor},
    commands::resolve_install_path,
    constants::{VERSION_INSTALL_MARKER, VERSION_INSTALL_MARKER_CONTENT},
    error::join_err_to_io_error,
    prelude::*,
    shell_utils::uninstall_path_configuration,
};

#[cfg(unix)]
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
                &managed_versions,
            )?;
            remove_all_versions(&versions_root, &managed_versions).await?;
            if let Err(e) = uninstall_path_configuration(&configured_target_dir) {
                tracing::warn!(error = %e.to_string(), "Failed to update shell rc files during --all removal");
            }
            remove_managed_shell_scripts(&install_root, &configured_target_dir)?;
            remove_open_dir_if_empty(versions_root)?;
            cleanup_managed_root_entries(&install_root, &target_dir, &managed_links)?;
            remove_open_dir_if_empty(install_root)?;
            tracing::info!("All versions and configuration removed successfully");
            return Ok(());
        }

        let managed_versions_before = managed_versions(&versions_root)?;
        let managed_links_before = managed_root_links(
            &install_root,
            &versions_root,
            &target_dir,
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
        let latest_version = latest_managed_version(&versions_root)?;

        if latest_version.is_none() {
            tracing::debug!("No versions remaining, cleaning up configuration");
            if let Err(e) = uninstall_path_configuration(&configured_target_dir) {
                tracing::warn!(error = %e.to_string(), "Failed to update shell rc files when cleaning up last version");
            }
            remove_managed_shell_scripts(&install_root, &configured_target_dir)?;
            remove_open_dir_if_empty(versions_root)?;
            cleanup_managed_root_entries(&install_root, &target_dir, &managed_links_before)?;
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
                &managed_links_before,
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

fn normalize_absolute_path(path: &Path) -> io::Result<PathBuf> {
    let absolute = std::path::absolute(path)?;
    let root = absolute
        .ancestors()
        .filter(|ancestor| ancestor.has_root())
        .last()
        .ok_or_else(|| io::Error::new(ErrorKind::InvalidInput, "path has no filesystem root"))?;
    let relative = absolute.strip_prefix(root).map_err(|_| {
        io::Error::new(
            ErrorKind::InvalidInput,
            "path could not be made relative to its filesystem root",
        )
    })?;
    let mut components = Vec::new();

    for component in relative.components() {
        match component {
            std::path::Component::Normal(name) => components.push(name.to_os_string()),
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                components.pop().ok_or_else(|| {
                    io::Error::new(ErrorKind::InvalidInput, "path escapes its filesystem root")
                })?;
            }
            std::path::Component::Prefix(_) | std::path::Component::RootDir => {
                return Err(io::Error::new(
                    ErrorKind::InvalidInput,
                    "unexpected root component in path",
                ));
            }
        }
    }

    let mut normalized = root.to_path_buf();
    normalized.extend(components);
    Ok(normalized)
}

fn open_install_root(path: &Path) -> io::Result<Dir> {
    let root = path
        .ancestors()
        .filter(|ancestor| ancestor.has_root())
        .last()
        .ok_or_else(|| io::Error::new(ErrorKind::InvalidInput, "path has no filesystem root"))?;
    let relative = path.strip_prefix(root).map_err(|_| {
        io::Error::new(
            ErrorKind::InvalidInput,
            "path could not be made relative to its filesystem root",
        )
    })?;
    let mut current = Dir::open_ambient_dir(root, ambient_authority())?;
    let mut opened_component = false;

    for component in relative.components() {
        let std::path::Component::Normal(name) = component else {
            return Err(io::Error::new(
                ErrorKind::InvalidInput,
                "install path must contain only normalized components",
            ));
        };
        current = open_cap_dir_nofollow(&current, Path::new(name))?;
        opened_component = true;
    }

    if !opened_component {
        return Err(io::Error::new(
            ErrorKind::InvalidInput,
            "the install root must not be a filesystem root",
        ));
    }

    Ok(current)
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
    if cap_file_matches(
        &version_dir,
        VERSION_INSTALL_MARKER,
        VERSION_INSTALL_MARKER_CONTENT,
    )? {
        return Ok(Some(version_dir));
    }

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

fn cap_file_matches(dir: &Dir, name: &str, expected: &str) -> io::Result<bool> {
    let metadata = match dir.symlink_metadata(name) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(error),
    };
    if metadata.file_type().is_symlink()
        || !metadata.is_file()
        || metadata.len() != expected.len() as u64
    {
        return Ok(false);
    }

    let mut content = Vec::with_capacity(expected.len() + 1);
    dir.open(name)?
        .take(expected.len() as u64 + 1)
        .read_to_end(&mut content)?;
    Ok(content == expected.as_bytes())
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
    create_missing: bool,
) -> Result<()> {
    for name in MANAGED_ROOT_LINKS {
        let display_path = target_dir.join(name);
        let managed_link = managed_links.iter().find(|link| link.name == name);
        let replace = managed_link.is_some_and(|link| link.version == *removed_version);

        if !replace && !(create_missing && managed_link.is_none()) {
            tracing::debug!(path = %display_path.display(), "Preserving unmanaged install-root entry");
            continue;
        }

        let removed = match managed_link {
            Some(link) if replace => quarantine_managed_root_link(install_root, link)?,
            _ => false,
        };
        if !removed {
            if create_missing
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
        if let Some(version) =
            root_link_target_version(versions_root, target_dir, name, &target, managed_versions)?
        {
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
    name: &str,
    link_target: &Path,
    managed_versions: &[Version],
) -> Result<Option<Version>> {
    if let Some(version) = lexical_root_link_target_version(target_dir, name, link_target) {
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
    name: &str,
    link_target: &Path,
) -> Option<Version> {
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
    if !link_target.is_absolute() || !link_name.eq_ignore_ascii_case(name) {
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

fn quarantine_entry(install_root: &Dir, name: &str) -> io::Result<Option<cap_tempfile::TempDir>> {
    let quarantine = cap_tempfile::tempdir_in(install_root)?;
    match install_root.rename(name, &quarantine, "entry") {
        Ok(()) => Ok(Some(quarantine)),
        Err(error) if error.kind() == ErrorKind::NotFound => {
            quarantine.close()?;
            Ok(None)
        }
        Err(error) => {
            quarantine.close()?;
            Err(error)
        }
    }
}

fn restore_quarantined_entry(
    quarantine: cap_tempfile::TempDir,
    install_root: &Dir,
    name: &str,
) -> io::Result<()> {
    if let Err(error) = rename_noreplace(
        &quarantine,
        Path::new("entry"),
        install_root,
        Path::new(name),
    ) {
        std::mem::forget(quarantine);
        return Err(io::Error::new(
            error.kind(),
            format!(
                "install-root entry changed during cleanup; the original {name} entry was \
                 preserved in an internal quarantine directory: {error}"
            ),
        ));
    }
    quarantine.close()
}

#[cfg(any(target_os = "linux", target_os = "android", target_vendor = "apple"))]
fn rename_noreplace(from_dir: &Dir, from: &Path, to_dir: &Dir, to: &Path) -> io::Result<()> {
    rustix::fs::renameat_with(
        from_dir,
        from,
        to_dir,
        to,
        rustix::fs::RenameFlags::NOREPLACE,
    )
    .map_err(io::Error::from)
}

#[cfg(windows)]
fn rename_noreplace(from_dir: &Dir, from: &Path, to_dir: &Dir, to: &Path) -> io::Result<()> {
    use windows_sys::Win32::Storage::FileSystem::MoveFileExW;

    let from = windows_child_path(from_dir, from)?;
    let to = windows_child_path(to_dir, to)?;

    // SAFETY: Both paths are NUL-terminated UTF-16 buffers that remain alive for the call.
    // Passing no flags gives MoveFileExW no replace-existing permission, so a concurrent
    // destination entry makes the operation fail instead of being overwritten.
    if unsafe { MoveFileExW(from.as_ptr(), to.as_ptr(), 0) } == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

#[cfg(windows)]
fn windows_child_path(dir: &Dir, child: &Path) -> io::Result<Vec<u16>> {
    use std::os::windows::{ffi::OsStrExt, io::AsRawHandle};
    use windows_sys::Win32::Storage::FileSystem::GetFinalPathNameByHandleW;

    let mut components = child.components();
    let Some(std::path::Component::Normal(name)) = components.next() else {
        return Err(io::Error::new(
            ErrorKind::InvalidInput,
            "rename path must be one normal component",
        ));
    };
    if components.next().is_some() {
        return Err(io::Error::new(
            ErrorKind::InvalidInput,
            "rename path must be one normal component",
        ));
    }

    let mut path = vec![0_u16; 512];
    loop {
        // SAFETY: `path` is a writable UTF-16 buffer and the directory handle remains valid.
        let written = unsafe {
            GetFinalPathNameByHandleW(
                dir.as_raw_handle(),
                path.as_mut_ptr(),
                path.len().try_into().map_err(|_| {
                    io::Error::new(ErrorKind::InvalidInput, "directory path is too long")
                })?,
                0,
            )
        };
        if written == 0 {
            return Err(io::Error::last_os_error());
        }
        if (written as usize) < path.len() {
            path.truncate(written as usize);
            break;
        }
        path.resize(written as usize + 1, 0);
    }

    if !path.ends_with(&[b'\\' as u16]) {
        path.push(b'\\' as u16);
    }
    path.extend(name.encode_wide());
    path.push(0);
    Ok(path)
}

#[cfg(all(
    unix,
    not(any(target_os = "linux", target_os = "android", target_vendor = "apple"))
))]
fn rename_noreplace(_from_dir: &Dir, _from: &Path, _to_dir: &Dir, _to: &Path) -> io::Result<()> {
    Err(io::Error::new(
        ErrorKind::Unsupported,
        "atomic no-replace rename is unavailable on this platform",
    ))
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
fn remove_managed_shell_scripts(install_root: &Dir, configured_target_dir: &Path) -> Result<()> {
    for name in ["env", "env.fish", "env.nu"] {
        let Some(expected) = managed_shell_script_content(name, configured_target_dir) else {
            continue;
        };
        let Some(quarantine) = quarantine_entry(install_root, name)? else {
            continue;
        };
        let owned = cap_file_matches(&quarantine, "entry", &expected)?;

        if owned {
            quarantine.remove_file("entry")?;
            quarantine.close()?;
        } else {
            restore_quarantined_entry(quarantine, install_root, name)?;
            tracing::debug!(%name, "Preserving unowned env script");
        }
    }

    Ok(())
}

#[cfg(windows)]
fn remove_managed_shell_scripts(_install_root: &Dir, _configured_target_dir: &Path) -> Result<()> {
    Ok(())
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

        remove_managed_shell_scripts(&install_root, &install_path).unwrap();

        assert!(outside_script.exists(), "outside env script must remain");
        assert!(
            !renamed_path.join("env").exists(),
            "managed script must be removed through the pinned root"
        );
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

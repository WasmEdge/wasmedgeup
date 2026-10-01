use crate::prelude::*;
use cap_primitives::fs::FollowSymlinks;
use cap_std::{ambient_authority, fs::Dir};
use snafu::ResultExt;

use std::io::{self, ErrorKind, Seek};
use std::path::PathBuf;

#[cfg(unix)]
use std::os::unix::fs::symlink as symlink_unix;

use std::path::Path;

#[cfg(windows)]
use std::os::windows::fs::{symlink_dir, symlink_file};

use std::fs::OpenOptions;
use tempfile::{Builder, TempDir};
use tokio::fs;
use walkdir::WalkDir;

pub(crate) fn open_dir_nofollow(path: &Path) -> io::Result<Dir> {
    walk_dir_nofollow(path, false)
}

pub(crate) fn open_or_create_dir_nofollow(path: &Path) -> io::Result<Dir> {
    walk_dir_nofollow(path, true)
}

fn walk_dir_nofollow(path: &Path, create_missing: bool) -> io::Result<Dir> {
    let path = resolve_trusted_root_alias(path);
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
                "path must contain only normalized components",
            ));
        };
        let name = Path::new(name);
        current = if create_missing {
            open_or_create_cap_dir_nofollow(&current, name)?
        } else {
            open_cap_dir_nofollow(&current, name)?
        };
        opened_component = true;
    }

    if !opened_component {
        return Err(io::Error::new(
            ErrorKind::InvalidInput,
            "the managed directory must not be a filesystem root",
        ));
    }

    Ok(current)
}

#[cfg(target_vendor = "apple")]
pub(crate) fn resolve_trusted_root_alias(path: &Path) -> PathBuf {
    // macOS exposes these root-owned aliases on every normal installation.
    // They are outside the user-controlled install tree and cannot be replaced
    // without root access. Resolve only the exact built-in target; every other
    // symlink component is still rejected by the capability walk below.
    for (alias, link_target, resolved) in [
        (
            Path::new("/var"),
            Path::new("private/var"),
            Path::new("/private/var"),
        ),
        (
            Path::new("/tmp"),
            Path::new("private/tmp"),
            Path::new("/private/tmp"),
        ),
        (
            Path::new("/etc"),
            Path::new("private/etc"),
            Path::new("/private/etc"),
        ),
    ] {
        let Ok(suffix) = path.strip_prefix(alias) else {
            continue;
        };
        if std::fs::read_link(alias).ok().as_deref() == Some(link_target) {
            return resolved.join(suffix);
        }
    }
    path.to_path_buf()
}

#[cfg(not(target_vendor = "apple"))]
pub(crate) fn resolve_trusted_root_alias(path: &Path) -> PathBuf {
    path.to_path_buf()
}

pub(crate) fn open_cap_dir_nofollow(parent: &Dir, path: &Path) -> io::Result<Dir> {
    let parent_file = parent.try_clone()?.into_std_file();
    let dir = cap_primitives::fs::open_dir_nofollow(&parent_file, path)?;
    Ok(Dir::from_std_file(dir))
}

#[cfg(windows)]
pub(crate) fn open_cap_dir_nofollow_for_rename(parent: &Dir, path: &Path) -> io::Result<Dir> {
    use cap_primitives::fs::{MetadataExt, OpenOptionsExt};
    use windows_sys::Win32::{
        Foundation::GENERIC_READ,
        Storage::FileSystem::{
            DELETE, FILE_ATTRIBUTE_REPARSE_POINT, FILE_FLAG_BACKUP_SEMANTICS, FILE_SHARE_READ,
            FILE_SHARE_WRITE,
        },
    };

    // Keep the normal cap-std directory invariant (no FILE_SHARE_DELETE), but
    // request DELETE on this private staging directory so the exact handle can
    // publish itself without ever reopening its name.
    let mut options = cap_std::fs::OpenOptions::new();
    options
        .access_mode(GENERIC_READ | DELETE)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS)
        ._cap_fs_ext_maybe_dir(true)
        ._cap_fs_ext_follow(FollowSymlinks::No);
    let file = parent.open_with(path, &options)?;
    let metadata = file.metadata()?;
    if !metadata.is_dir() || metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
        return Err(io::Error::new(
            ErrorKind::NotADirectory,
            "rename source must be a real directory",
        ));
    }
    Ok(Dir::from_std_file(file.into_std()))
}

#[cfg(windows)]
pub(crate) fn open_cap_dir_identity_nofollow(
    parent: &Dir,
    path: &Path,
) -> io::Result<std::fs::File> {
    use cap_primitives::fs::{MetadataExt, OpenOptionsExt};
    use windows_sys::Win32::Storage::FileSystem::{
        FILE_ATTRIBUTE_REPARSE_POINT, FILE_FLAG_BACKUP_SEMANTICS, FILE_SHARE_DELETE,
        FILE_SHARE_READ, FILE_SHARE_WRITE,
    };

    // This handle is only used for identity comparison. Sharing deletion keeps
    // it compatible with the DELETE-capable publication handle, which remains
    // open and prevents the published directory from being renamed.
    let mut options = cap_std::fs::OpenOptions::new();
    options
        .access_mode(0)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS)
        ._cap_fs_ext_maybe_dir(true)
        ._cap_fs_ext_follow(FollowSymlinks::No);
    let file = parent.open_with(path, &options)?;
    let metadata = file.metadata()?;
    if !metadata.is_dir() || metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
        return Err(io::Error::new(
            ErrorKind::NotADirectory,
            "identity target must be a real directory",
        ));
    }
    Ok(file.into_std())
}

pub(crate) fn open_or_create_cap_dir_nofollow(parent: &Dir, path: &Path) -> io::Result<Dir> {
    match open_cap_dir_nofollow(parent, path) {
        Ok(dir) => Ok(dir),
        Err(error) if error.kind() == ErrorKind::NotFound => {
            match parent.create_dir(path) {
                Ok(()) => {}
                Err(error) if error.kind() == ErrorKind::AlreadyExists => {}
                Err(error) => return Err(error),
            }
            open_cap_dir_nofollow(parent, path)
        }
        Err(error) => Err(error),
    }
}

pub(crate) fn open_or_create_cap_dir_path_nofollow(root: &Dir, path: &Path) -> io::Result<Dir> {
    let mut current = root.try_clone()?;

    for component in path.components() {
        let std::path::Component::Normal(name) = component else {
            return Err(io::Error::new(
                ErrorKind::InvalidInput,
                "path must contain only normalized components",
            ));
        };
        current = open_or_create_cap_dir_nofollow(&current, Path::new(name))?;
    }

    Ok(current)
}

pub(crate) fn cap_version_has_runtime(version_dir: &Dir) -> io::Result<bool> {
    let bin_metadata = match version_dir.symlink_metadata("bin") {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(error),
    };
    if bin_metadata.file_type().is_symlink() || !bin_metadata.is_dir() {
        return Ok(false);
    }

    let bin_dir = open_cap_dir_nofollow(version_dir, Path::new("bin"))?;
    for binary in ["wasmedge", "wasmedge.exe"] {
        match bin_dir.symlink_metadata(binary) {
            Ok(metadata) if metadata.is_file() && !metadata.file_type().is_symlink() => {
                return Ok(true);
            }
            Ok(_) => {}
            Err(error) if error.kind() == ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
    }

    Ok(false)
}

pub(crate) fn can_write_to_cap_directory(dir: &Dir) -> bool {
    cap_tempfile::TempFile::new_anonymous(dir).is_ok()
}

#[cfg(unix)]
pub(crate) fn sync_cap_directory(dir: &Dir) -> io::Result<()> {
    use rustix::fs::{fsync, openat, Mode, OFlags};

    // cap-std may represent a directory with an O_PATH descriptor on Linux,
    // which cannot itself be fsynced. Reopen `.` relative to the pinned handle
    // as a readable directory so the rename is made durable without returning
    // to an ambient pathname.
    let sync_fd = openat(
        dir,
        Path::new("."),
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC,
        Mode::empty(),
    )
    .map_err(io::Error::from)?;
    fsync(&sync_fd).map_err(io::Error::from)
}

pub(crate) fn quarantine_entry(
    managed_root: &Dir,
    name: &str,
) -> io::Result<Option<cap_tempfile::TempDir>> {
    quarantine_entry_in(managed_root, name, managed_root)
}

fn quarantine_entry_in(
    managed_root: &Dir,
    name: &str,
    quarantine_root: &Dir,
) -> io::Result<Option<cap_tempfile::TempDir>> {
    let quarantine = cap_tempfile::tempdir_in(quarantine_root)?;
    match managed_root.rename(name, &quarantine, "entry") {
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

pub(crate) fn restore_quarantined_entry(
    quarantine: cap_tempfile::TempDir,
    managed_root: &Dir,
    name: &str,
) -> io::Result<()> {
    if let Err(error) = rename_noreplace(
        &quarantine,
        Path::new("entry"),
        managed_root,
        Path::new(name),
    ) {
        std::mem::forget(quarantine);
        return Err(io::Error::new(
            error.kind(),
            format!(
                "managed entry changed during cleanup; the original {name} entry was preserved \
                 in an internal quarantine directory: {error}"
            ),
        ));
    }
    quarantine.close()
}

#[cfg(any(target_os = "linux", target_os = "android", target_vendor = "apple"))]
pub(crate) fn rename_noreplace(
    from_dir: &Dir,
    from: &Path,
    to_dir: &Dir,
    to: &Path,
) -> io::Result<()> {
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
pub(crate) fn rename_noreplace(
    from_dir: &Dir,
    from: &Path,
    to_dir: &Dir,
    to: &Path,
) -> io::Result<()> {
    use cap_primitives::fs::OpenOptionsExt;
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Storage::FileSystem::{DELETE, FILE_FLAG_BACKUP_SEMANTICS};

    windows_single_component(from)?;
    // Open the source relative to the pinned directory with DELETE access. The
    // resulting handle pins the exact entry even if its name is replaced before
    // the handle-relative rename runs.
    let mut options = cap_std::fs::OpenOptions::new();
    options
        .access_mode(DELETE)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS)
        ._cap_fs_ext_maybe_dir(true)
        ._cap_fs_ext_follow(FollowSymlinks::No);
    let source = from_dir.open_with(from, &options)?;

    rename_noreplace_handle(source.as_raw_handle(), to_dir, to)
}

#[cfg(windows)]
pub(crate) fn rename_noreplace_pinned_dir(source: Dir, to_dir: &Dir, to: &Path) -> io::Result<Dir> {
    use std::os::windows::io::AsRawHandle;

    rename_noreplace_handle(source.as_raw_handle(), to_dir, to)?;
    Ok(source)
}

#[cfg(windows)]
fn rename_noreplace_handle(
    source: std::os::windows::io::RawHandle,
    to_dir: &Dir,
    to: &Path,
) -> io::Result<()> {
    use std::os::windows::io::AsRawHandle;
    use std::{mem::size_of, os::windows::ffi::OsStrExt};
    use windows_sys::{
        Wdk::Storage::FileSystem::{
            FileRenameInformation, NtSetInformationFile, FILE_RENAME_INFORMATION,
        },
        Win32::{Foundation::RtlNtStatusToDosError, System::IO::IO_STATUS_BLOCK},
    };

    let to_name = windows_single_component(to)?;
    let to_wide = to_name.encode_wide().collect::<Vec<_>>();
    let file_name_bytes = to_wide
        .len()
        .checked_mul(size_of::<u16>())
        .and_then(|length| u32::try_from(length).ok())
        .ok_or_else(|| io::Error::new(ErrorKind::InvalidInput, "rename name is too long"))?;

    // FILE_RENAME_INFORMATION has a flexible UTF-16 tail. Back it with usize words so
    // the structure remains pointer-aligned, and use the pinned destination
    // directory as RootDirectory so no ambient pathname is reconstructed.
    let buffer_bytes = size_of::<FILE_RENAME_INFORMATION>()
        .checked_add(file_name_bytes as usize)
        .ok_or_else(|| io::Error::new(ErrorKind::InvalidInput, "rename name is too long"))?;
    let buffer_bytes_u32 = u32::try_from(buffer_bytes)
        .map_err(|_| io::Error::new(ErrorKind::InvalidInput, "rename name is too long"))?;
    let buffer_words = buffer_bytes.div_ceil(size_of::<usize>());
    let mut buffer = vec![0_usize; buffer_words];
    let rename_info = buffer.as_mut_ptr().cast::<FILE_RENAME_INFORMATION>();
    let mut io_status = IO_STATUS_BLOCK::default();

    // SAFETY: `buffer` is pointer-aligned and large enough for the fixed
    // FILE_RENAME_INFORMATION fields plus every UTF-16 code unit. Both handles and the
    // backing buffer remain alive for the call. ReplaceIfExists=false gives the
    // operation atomic no-replace semantics. NtSetInformationFile accepts the
    // destination name relative to RootDirectory without reconstructing a path.
    let status = unsafe {
        (*rename_info).Anonymous.ReplaceIfExists = false;
        (*rename_info).RootDirectory = to_dir.as_raw_handle();
        (*rename_info).FileNameLength = file_name_bytes;
        std::ptr::copy_nonoverlapping(
            to_wide.as_ptr(),
            std::ptr::addr_of_mut!((*rename_info).FileName).cast::<u16>(),
            to_wide.len(),
        );
        NtSetInformationFile(
            source,
            &mut io_status,
            rename_info.cast(),
            buffer_bytes_u32,
            FileRenameInformation,
        )
    };
    if status < 0 {
        let win32_error = unsafe { RtlNtStatusToDosError(status) };
        return Err(io::Error::from_raw_os_error(win32_error as i32));
    }
    Ok(())
}

#[cfg(windows)]
fn windows_single_component(path: &Path) -> io::Result<&std::ffi::OsStr> {
    use std::os::windows::ffi::OsStrExt;

    let mut components = path.components();
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
    if name
        .encode_wide()
        .any(|unit| matches!(unit, 0 | 47 | 58 | 92))
    {
        return Err(io::Error::new(
            ErrorKind::InvalidInput,
            "rename path must be one safe Windows filename",
        ));
    }
    Ok(name)
}

#[cfg(all(
    unix,
    not(any(target_os = "linux", target_os = "android", target_vendor = "apple"))
))]
pub(crate) fn rename_noreplace(
    _from_dir: &Dir,
    _from: &Path,
    _to_dir: &Dir,
    _to: &Path,
) -> io::Result<()> {
    Err(io::Error::new(
        ErrorKind::Unsupported,
        "atomic no-replace rename is unavailable on this platform",
    ))
}

/// Create an isolated temporary workspace with an unpredictable name under
/// `base`.
///
/// Staging an install in a deterministic `base/<install_name>` directory
/// created with `create_dir_all` is unsafe on a shared temp filesystem: a
/// local attacker can predict that path and pre-create it (or symlink it
/// elsewhere) before a privileged install, redirecting download and extract
/// writes outside the intended boundary (CWE-59 / CWE-377). The guarantee
/// `tempfile` provides is an *exclusive* directory create: `tempdir_in` issues
/// a `mkdir`-style create that fails with `EEXIST` and retries a fresh name, so
/// a directory or symlink an attacker pre-creates at the chosen path is never
/// adopted or followed. The randomized name is defense-in-depth that makes the
/// path hard to guess in the first place — but note `tempfile`'s RNG is not
/// cryptographic, so the safety rests on the exclusive create, not on secrecy
/// of the name (don't replace this with a predictable name + plain
/// `create_dir_all`). The returned [`TempDir`] removes itself on drop, cleaning
/// up even when a later install step fails.
pub fn create_temp_workspace(base: &Path, install_name: &str) -> Result<TempDir> {
    // `install_name` is used verbatim as the tempfile name prefix, which
    // `tempdir_in` joins onto `base`. Anything other than a single *normal*
    // path component would let the workspace be created outside `base`
    // (plugin names are unvalidated user input), defeating the containment
    // this helper provides, so require exactly one normal component. A bare
    // separator check is not enough: `../../evil` escapes via `..`, and on
    // Windows a drive-relative prefix like `C:evil` (which
    // `std::path::is_separator` does *not* flag) makes `base.join(..)` discard
    // `base` entirely. Reject all of those rather than silently escaping the
    // staging boundary.
    let mut components = Path::new(install_name).components();
    let is_single_normal_component = matches!(
        components.next(),
        Some(std::path::Component::Normal(name)) if name == std::ffi::OsStr::new(install_name)
    ) && components.next().is_none();
    if !is_single_normal_component {
        return Err(Error::InvalidPath {
            path: install_name.to_string(),
            reason: "temp workspace name must be a single path component".to_string(),
        });
    }

    std::fs::create_dir_all(base).map_err(|source| Error::Io {
        action: "create temp workspace base directory".to_string(),
        path: base.display().to_string(),
        source,
    })?;

    let prefix = format!("{install_name}-");
    let mut builder = Builder::new();
    builder.prefix(&prefix);
    // Stage privileged, not-yet-verified downloads in a private directory so
    // other local users on a shared temp filesystem cannot read them; tempfile
    // otherwise creates the workspace with the process umask (typically 0755).
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        builder.permissions(std::fs::Permissions::from_mode(0o700));
    }
    let workspace = builder.tempdir_in(base).map_err(|source| Error::Io {
        action: "create temp workspace".to_string(),
        path: base.display().to_string(),
        source,
    })?;
    Ok(workspace)
}

pub fn can_write_to_directory(path: &Path) -> bool {
    let test_file = path.join(".wasmedgeup_write_test");
    let can_write = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .open(&test_file)
        .is_ok();

    if test_file.exists() {
        let _ = std::fs::remove_file(test_file);
    }

    can_write
}

/// Copy every file and symlink reachable from `from_dir` into `to_dir`,
/// renaming any `lib64` path component to `lib` along the way.
///
/// # Semantics — "walk all, log all, return first"
///
/// `copy_tree` is **not atomic**. The walk continues past per-entry errors so
/// the [`tracing`] log captures the full set of failures (useful when the
/// support artifact for an installer is the user's log), but the function
/// returns `Err` with the **first** error it encountered as soon as the walk
/// finishes. The summary `failure_count` is logged at `error` level just
/// before returning.
///
/// Consequences for callers:
///
/// - On success (`Ok(())`), every entry copied cleanly.
/// - On failure (`Err(_)`), `to_dir` may be **partially populated** with
///   whatever entries succeeded before/after the failing ones. Callers that
///   need atomic install behavior should layer a tempdir-and-rename strategy
///   on top, or roll back `to_dir` themselves.
/// - The returned error is the first one chronologically; subsequent
///   failures are only visible via the log.
///
/// Both walker errors (e.g. permission denied descending into a subdir) and
/// per-entry errors (failed metadata read, failed copy, failed symlink
/// removal/creation) are counted and considered for `first_error`.
pub async fn copy_tree(from_dir: &Path, to_dir: &Path) -> Result<()> {
    let mut first_error: Option<Error> = None;
    let mut failure_count: usize = 0;

    // Walk explicitly: WalkDir yields Result<DirEntry, walkdir::Error> and
    // dropping the Err arm via filter_map(|e| e.ok()) would re-introduce the
    // exact silent-failure pattern this fix exists to eliminate (permission
    // denied while reading a subdir, broken loop detection, etc.).
    for result in WalkDir::new(from_dir) {
        match result {
            Ok(entry) => {
                if let Err(e) = copy_entry(&entry, from_dir, to_dir).await {
                    tracing::warn!(
                        error = %e,
                        entry = %entry.path().display(),
                        "copy_tree entry failed"
                    );
                    failure_count += 1;
                    if first_error.is_none() {
                        first_error = Some(e);
                    }
                }
            }
            Err(walk_err) => {
                // Snapshot the original walkdir error message before
                // into_io_error() consumes it; this preserves loop-detection
                // and other non-IO walkdir variants in the fallback message
                // that would otherwise be replaced by a generic placeholder.
                let walk_msg = walk_err.to_string();
                let path = walk_err
                    .path()
                    .map(|p| p.display().to_string())
                    .unwrap_or_else(|| from_dir.display().to_string());
                let source = walk_err
                    .into_io_error()
                    .unwrap_or_else(|| std::io::Error::other(walk_msg));
                let e = Error::Io {
                    action: "walk source tree".to_string(),
                    path: path.clone(),
                    source,
                };
                tracing::warn!(
                    error = %e,
                    path = %path,
                    "copy_tree walk error"
                );
                failure_count += 1;
                if first_error.is_none() {
                    first_error = Some(e);
                }
            }
        }
    }

    if let Some(e) = first_error {
        tracing::error!(
            failure_count,
            "copy_tree finished with {failure_count} failure(s); returning the first",
        );
        return Err(e);
    }
    Ok(())
}

/// Copy an extracted runtime into an already-open destination directory.
///
/// All destination traversal is relative to `to_dir`, and every intermediate
/// directory is opened without following symlinks. Keeping this handle alive
/// prevents a concurrent rename of the user-supplied install path from
/// redirecting installer writes outside the directory that was validated.
pub(crate) async fn copy_tree_to_cap_dir(
    from_dir: &Path,
    to_dir: &Dir,
    to_dir_display: &Path,
) -> Result<()> {
    let from_dir = from_dir.to_path_buf();
    let to_dir = to_dir.try_clone()?;
    let to_dir_display = to_dir_display.to_path_buf();
    let destination_error_path = to_dir_display.display().to_string();

    tokio::task::spawn_blocking(move || {
        copy_tree_to_cap_dir_blocking(&from_dir, &to_dir, &to_dir_display)
    })
    .await
    .map_err(|join_error| Error::Io {
        action: "copy extracted runtime".to_string(),
        path: destination_error_path,
        source: crate::error::join_err_to_io_error(join_error),
    })?
}

/// Copy one regular file into an already-open destination directory without
/// returning to an ambient destination path.
pub(crate) async fn copy_file_to_cap_dir(
    source_path: &Path,
    destination_dir: &Dir,
    name: &std::ffi::OsStr,
    target_display: &Path,
) -> Result<()> {
    let source_path = source_path.to_path_buf();
    let destination_dir = destination_dir.try_clone()?;
    let name = name.to_os_string();
    let target_display = target_display.to_path_buf();
    let error_path = target_display.display().to_string();

    tokio::task::spawn_blocking(move || {
        let metadata = std::fs::symlink_metadata(&source_path).map_err(|source| Error::Io {
            action: "inspect plugin source file".to_string(),
            path: source_path.display().to_string(),
            source,
        })?;
        if metadata.file_type().is_symlink() || !metadata.is_file() {
            return Err(Error::InvalidPath {
                path: source_path.display().to_string(),
                reason: "plugin source must be a regular file, not a symlink or directory"
                    .to_string(),
            });
        }
        copy_file_entry_to_cap_dir(
            &source_path,
            &metadata,
            &destination_dir,
            &name,
            &target_display,
        )
    })
    .await
    .map_err(|join_error| Error::Io {
        action: "copy plugin file".to_string(),
        path: error_path,
        source: crate::error::join_err_to_io_error(join_error),
    })?
}

fn copy_tree_to_cap_dir_blocking(
    from_dir: &Path,
    to_dir: &Dir,
    to_dir_display: &Path,
) -> Result<()> {
    for result in WalkDir::new(from_dir) {
        let entry = result.map_err(|walk_error| {
            let message = walk_error.to_string();
            let path = walk_error
                .path()
                .map(|path| path.display().to_string())
                .unwrap_or_else(|| from_dir.display().to_string());
            let source = walk_error
                .into_io_error()
                .unwrap_or_else(|| io::Error::other(message));
            Error::Io {
                action: "walk source tree".to_string(),
                path,
                source,
            }
        })?;
        copy_entry_to_cap_dir(&entry, from_dir, to_dir, to_dir_display)?;
    }

    Ok(())
}

fn copy_entry_to_cap_dir(
    entry: &walkdir::DirEntry,
    from_dir: &Path,
    to_dir: &Dir,
    to_dir_display: &Path,
) -> Result<()> {
    let metadata = entry.metadata().map_err(|walk_error| {
        let message = walk_error.to_string();
        let source = walk_error
            .into_io_error()
            .unwrap_or_else(|| io::Error::other(message));
        Error::Io {
            action: "read entry metadata".to_string(),
            path: entry.path().display().to_string(),
            source,
        }
    })?;
    let relative = mapped_install_path(entry.path(), from_dir)?;
    if relative.as_os_str().is_empty() {
        return Ok(());
    }

    if metadata.is_dir() {
        open_or_create_cap_dir_path_nofollow(to_dir, &relative).map_err(|source| Error::Io {
            action: "create target directory".to_string(),
            path: to_dir_display.join(&relative).display().to_string(),
            source,
        })?;
        return Ok(());
    }
    if !metadata.is_file() && !metadata.is_symlink() {
        return Ok(());
    }

    let parent_relative = relative.parent().unwrap_or_else(|| Path::new(""));
    let parent =
        open_or_create_cap_dir_path_nofollow(to_dir, parent_relative).map_err(|source| {
            Error::Io {
                action: "create target parent directory".to_string(),
                path: to_dir_display.join(parent_relative).display().to_string(),
                source,
            }
        })?;
    let name = relative.file_name().ok_or_else(|| Error::InvalidPath {
        path: relative.display().to_string(),
        reason: "target has no file name".to_string(),
    })?;
    let target_display = to_dir_display.join(&relative);

    if metadata.is_symlink() {
        copy_symlink_entry_to_cap_dir(entry.path(), &parent, name, &target_display)
    } else {
        copy_file_entry_to_cap_dir(entry.path(), &metadata, &parent, name, &target_display)
    }
}

fn mapped_install_path(entry: &Path, from_dir: &Path) -> Result<PathBuf> {
    let relative = entry
        .strip_prefix(from_dir)
        .map_err(|_| Error::InvalidPath {
            path: entry.display().to_string(),
            reason: "archive entry is outside the extracted runtime root".to_string(),
        })?;
    let mut mapped = PathBuf::new();

    for component in relative.components() {
        let std::path::Component::Normal(name) = component else {
            return Err(Error::InvalidPath {
                path: relative.display().to_string(),
                reason: "archive entry path is not normalized".to_string(),
            });
        };
        if name == std::ffi::OsStr::new("lib64") {
            mapped.push(LIB_DIR);
        } else {
            mapped.push(name);
        }
    }

    Ok(mapped)
}

fn copy_file_entry_to_cap_dir(
    source_path: &Path,
    source_metadata: &std::fs::Metadata,
    parent: &Dir,
    name: &std::ffi::OsStr,
    target_display: &Path,
) -> Result<()> {
    match parent.symlink_metadata(name) {
        Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => {
            return Err(Error::InvalidPath {
                path: target_display.display().to_string(),
                reason: "refusing to replace an existing directory with a file".to_string(),
            });
        }
        Ok(_) => {}
        Err(error) if error.kind() == ErrorKind::NotFound => {}
        Err(source) => {
            return Err(Error::Io {
                action: "inspect target file".to_string(),
                path: target_display.display().to_string(),
                source,
            });
        }
    }

    let staging = cap_tempfile::tempdir_in(parent)?;
    let mut options = cap_std::fs::OpenOptions::new();
    options
        .write(true)
        .create_new(true)
        ._cap_fs_ext_follow(FollowSymlinks::No);
    let mut destination = staging
        .open_with("entry", &options)
        .map_err(|source| Error::Io {
            action: "create staged target file".to_string(),
            path: target_display.display().to_string(),
            source,
        })?
        .into_std();
    let mut source = std::fs::File::open(source_path).map_err(|source| Error::Io {
        action: "open source file".to_string(),
        path: source_path.display().to_string(),
        source,
    })?;
    io::copy(&mut source, &mut destination).map_err(|source| Error::Io {
        action: "copy file".to_string(),
        path: format!("{} -> {}", source_path.display(), target_display.display()),
        source,
    })?;
    destination
        .set_permissions(source_metadata.permissions())
        .map_err(|source| Error::Io {
            action: "set target file permissions".to_string(),
            path: target_display.display().to_string(),
            source,
        })?;
    destination.sync_all().map_err(|source| Error::Io {
        action: "sync target file".to_string(),
        path: target_display.display().to_string(),
        source,
    })?;
    drop(destination);

    #[cfg(windows)]
    remove_cap_entry(parent, name, target_display)?;
    staging
        .rename("entry", parent, name)
        .map_err(|source| Error::Io {
            action: "publish target file".to_string(),
            path: target_display.display().to_string(),
            source,
        })?;
    if let Err(error) = staging.close() {
        tracing::warn!(%error, path = %target_display.display(), "Failed to remove empty file staging directory");
    }
    Ok(())
}

fn copy_symlink_entry_to_cap_dir(
    source_path: &Path,
    parent: &Dir,
    name: &std::ffi::OsStr,
    target_display: &Path,
) -> Result<()> {
    let link_target = std::fs::read_link(source_path).map_err(|source| Error::Io {
        action: "read symlink target".to_string(),
        path: source_path.display().to_string(),
        source,
    })?;
    if parent
        .symlink_metadata(name)
        .is_ok_and(|metadata| metadata.is_dir() && !metadata.file_type().is_symlink())
    {
        return Err(Error::InvalidPath {
            path: target_display.display().to_string(),
            reason: "refusing to replace an existing directory with a symlink".to_string(),
        });
    }
    let staging = cap_tempfile::tempdir_in(parent)?;

    #[cfg(unix)]
    staging
        .symlink_contents(&link_target, "entry")
        .map_err(|source| Error::Io {
            action: "create staged symlink".to_string(),
            path: target_display.display().to_string(),
            source,
        })?;

    #[cfg(windows)]
    {
        let is_dir = std::fs::metadata(source_path)
            .map(|metadata| metadata.is_dir())
            .unwrap_or(false);
        let result = if is_dir {
            staging.symlink_dir(&link_target, "entry")
        } else {
            staging.symlink_file(&link_target, "entry")
        };
        result.map_err(|source| {
            if source.kind() == ErrorKind::PermissionDenied {
                Error::WindowsSymlinkError {
                    version: std::env::var("WASMEDGE_VERSION")
                        .unwrap_or_else(|_| "latest".to_string()),
                }
            } else {
                Error::Io {
                    action: "create symlink".to_string(),
                    path: target_display.display().to_string(),
                    source,
                }
            }
        })?;
    }

    #[cfg(windows)]
    remove_cap_entry(parent, name, target_display)?;
    staging
        .rename("entry", parent, name)
        .map_err(|source| Error::Io {
            action: "publish target symlink".to_string(),
            path: target_display.display().to_string(),
            source,
        })?;
    if let Err(error) = staging.close() {
        tracing::warn!(%error, path = %target_display.display(), "Failed to remove empty symlink staging directory");
    }
    Ok(())
}

#[cfg(windows)]
fn remove_cap_entry(parent: &Dir, name: &std::ffi::OsStr, target_display: &Path) -> Result<()> {
    let metadata = match parent.symlink_metadata(name) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(()),
        Err(source) => {
            return Err(Error::Io {
                action: "inspect existing target".to_string(),
                path: target_display.display().to_string(),
                source,
            });
        }
    };

    if metadata.is_dir() && !metadata.file_type().is_symlink() {
        return Err(Error::InvalidPath {
            path: target_display.display().to_string(),
            reason: "refusing to replace an existing directory".to_string(),
        });
    }

    if metadata.file_type().is_symlink() && parent.remove_dir(name).is_ok() {
        return Ok(());
    }

    parent.remove_file(name).map_err(|source| Error::Io {
        action: "remove existing target".to_string(),
        path: target_display.display().to_string(),
        source,
    })?;
    Ok(())
}

/// Copy or symlink a single walkdir entry into `to_dir`, mapping `lib64` to
/// `lib` along the way. Directories are skipped (the walker walks into them
/// and emits files/symlinks separately); any I/O failure returns a typed
/// error so `copy_tree` can surface partial installs instead of silently
/// succeeding.
async fn copy_entry(entry: &walkdir::DirEntry, from_dir: &Path, to_dir: &Path) -> Result<()> {
    tracing::trace!(entry = %entry.path().display(), "Copying entry");

    // walkdir::Error wraps an optional io::Error; preserve it (kind /
    // raw_os_error) instead of stringifying so callers downstream can match
    // on ErrorKind::PermissionDenied etc. For non-IO walkdir variants
    // (loop detection etc.) keep the original message in the fallback.
    let metadata = entry.metadata().map_err(|e| {
        let walk_msg = e.to_string();
        let path = entry.path().display().to_string();
        let source = e
            .into_io_error()
            .unwrap_or_else(|| std::io::Error::other(walk_msg));
        Error::Io {
            action: "read entry metadata".to_string(),
            path,
            source,
        }
    })?;
    if !metadata.is_file() && !metadata.is_symlink() {
        return Ok(());
    }

    // Calculate the target location by stripping the source directory
    // prefix from the entry path and appending it to the destination.
    // During this process, any `lib64` path component is renamed to
    // `lib` for consistency.
    //
    // Example:
    //   from_dir = '/from/path'
    //   entry    = '/from/path/foo/lib64/something.so'
    //   to_dir   = '/to/path'
    //   result   = '/to/path/foo/lib/something.so'
    let target_loc = to_dir.join(
        entry
            .path()
            .strip_prefix(from_dir)
            .unwrap_or(entry.path())
            .to_string_lossy()
            .replace("lib64", LIB_DIR),
    );

    let parent = target_loc.parent().ok_or_else(|| Error::InvalidPath {
        path: target_loc.display().to_string(),
        reason: "target has no parent directory".to_string(),
    })?;
    fs::create_dir_all(parent)
        .await
        .map_err(|source| Error::Io {
            action: "create target parent directory".to_string(),
            path: parent.display().to_string(),
            source,
        })?;

    if metadata.is_symlink() {
        copy_symlink_entry(entry.path(), &target_loc).await
    } else {
        fs::copy(entry.path(), &target_loc)
            .await
            .map_err(|source| Error::Io {
                action: "copy file".to_string(),
                path: format!(
                    "{src} -> {dst}",
                    src = entry.path().display(),
                    dst = target_loc.display(),
                ),
                source,
            })?;
        Ok(())
    }
}

/// Recreate a symlink from `src_link` (whose target we follow with
/// `read_link`) at `target_loc`, replacing any pre-existing entry.
async fn copy_symlink_entry(src_link: &Path, target_loc: &Path) -> Result<()> {
    let symlink_target = std::fs::read_link(src_link).map_err(|source| Error::Io {
        action: "read symlink target".to_string(),
        path: src_link.display().to_string(),
        source,
    })?;

    #[cfg(unix)]
    {
        remove_existing_symlink_unix(target_loc).await?;
        symlink_unix(&symlink_target, target_loc).map_err(|source| Error::Io {
            action: "create symlink".to_string(),
            path: target_loc.display().to_string(),
            source,
        })?;
        Ok(())
    }

    #[cfg(windows)]
    {
        // remove_existing_symlink_windows decides remove_dir vs remove_file
        // from the existing target's type. The create call still derives
        // is_dir from src_link because that's what dictates whether the
        // *new* entry should be symlink_dir vs symlink_file.
        remove_existing_symlink_windows(target_loc).await?;
        let is_dir = std::fs::metadata(src_link)
            .map(|m| m.is_dir())
            .unwrap_or(false);
        create_symlink_windows(&symlink_target, target_loc, is_dir)?;
        Ok(())
    }
}

#[cfg(unix)]
async fn remove_existing_symlink_unix(target_loc: &Path) -> Result<()> {
    // exists() follows symlinks, so a *broken* symlink would report false
    // and we'd skip the remove — the next symlink() then fails with EEXIST.
    // symlink_metadata reports on the link itself.
    let meta = match fs::symlink_metadata(target_loc).await {
        Ok(m) => m,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(source) => {
            return Err(Error::Io {
                action: "stat existing target".to_string(),
                path: target_loc.display().to_string(),
                source,
            })
        }
    };

    // symlink_metadata does not follow links, so meta.is_dir() is true only
    // for real directories — symlinks (file or dir) have is_dir=false and
    // is_symlink=true. A previous install that left a real directory at
    // this path needs remove_dir_all to be replaced cleanly; remove_file
    // would fail with EISDIR. remove_dir_all on Unix unlinks symlinks
    // without following them, so it stays safe for the symlink case too,
    // though we won't reach this branch for symlinks anyway.
    let result = if meta.is_dir() {
        fs::remove_dir_all(target_loc).await
    } else {
        fs::remove_file(target_loc).await
    };
    result.map_err(|source| Error::Io {
        action: "remove existing target".to_string(),
        path: target_loc.display().to_string(),
        source,
    })
}

#[cfg(windows)]
async fn remove_existing_symlink_windows(target_loc: &Path) -> Result<()> {
    // symlink_metadata so a broken directory-symlink (whose target was
    // deleted) is still detected and removed. Choose remove_dir_all vs
    // remove_file from the *existing* entry's type rather than from
    // src_link's type — they can differ when replacing a previous install
    // that happened to be a different file kind.
    let meta = match fs::symlink_metadata(target_loc).await {
        Ok(m) => m,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(source) => {
            return Err(Error::Io {
                action: "stat existing target".to_string(),
                path: target_loc.display().to_string(),
                source,
            })
        }
    };

    // remove_dir_all (vs remove_dir) so a previous install that left a real
    // non-empty directory at this path can still be replaced — remove_dir
    // would fail with DirectoryNotEmpty. Modern Rust's remove_dir_all
    // unlinks directory symlinks without recursing into the target, so
    // this is also safe for the dir-symlink case.
    let result = if meta.is_dir() {
        fs::remove_dir_all(target_loc).await
    } else {
        fs::remove_file(target_loc).await
    };
    result.map_err(|source| {
        if source.kind() == std::io::ErrorKind::PermissionDenied {
            Error::WindowsSymlinkError {
                version: std::env::var("WASMEDGE_VERSION").unwrap_or_else(|_| "latest".to_string()),
            }
        } else {
            Error::Io {
                action: "remove existing target".to_string(),
                path: target_loc.display().to_string(),
                source,
            }
        }
    })
}

#[cfg(windows)]
fn create_symlink_windows(target: &Path, link: &Path, is_dir: bool) -> Result<()> {
    let res = if is_dir {
        symlink_dir(target, link)
    } else {
        symlink_file(target, link)
    };
    res.map_err(|source| {
        if source.kind() == std::io::ErrorKind::PermissionDenied {
            Error::WindowsSymlinkError {
                version: std::env::var("WASMEDGE_VERSION").unwrap_or_else(|_| "latest".to_string()),
            }
        } else {
            Error::Io {
                action: "create symlink".to_string(),
                path: link.display().to_string(),
                source,
            }
        }
    })
}

/// Extract a compressed archive (`.tar.gz` on Unix, `.zip` on Windows) to
/// `dest`. The file ownership is consumed because the synchronous
/// extraction runs on a blocking worker via [`tokio::task::spawn_blocking`],
/// so the tokio main runtime stays free to make progress on other async
/// tasks while tar/zip decoding proceeds (unpacking a ~80MB runtime bundle
/// can take seconds).
pub async fn extract_archive(file: std::fs::File, dest: &Path) -> Result<()> {
    fs::create_dir_all(dest).await.inspect_err(
        |e| tracing::error!(error = %e.to_string(), "Failed to create directory during extraction"),
    )?;

    let dest_buf = dest.to_path_buf();
    match tokio::task::spawn_blocking(move || extract_archive_blocking(file, &dest_buf)).await {
        Ok(inner) => inner,
        Err(join_err) => Err(Error::Io {
            action: "archive extraction task".to_string(),
            path: dest.display().to_string(),
            source: crate::error::join_err_to_io_error(join_err),
        }),
    }
}

fn extract_archive_blocking(mut file: std::fs::File, dest: &Path) -> Result<()> {
    file.rewind()?;

    #[cfg(unix)]
    {
        use flate2::read::GzDecoder;
        let decompressed = GzDecoder::new(&mut file);
        extract_tar(decompressed, dest)?;
    }

    #[cfg(windows)]
    extract_zip(&mut file, dest)?;

    Ok(())
}

#[cfg(unix)]
fn extract_tar(file: impl std::io::Read, to: &Path) -> Result<()> {
    use tar::Archive;

    let mut archive = Archive::new(file);
    archive.unpack(to).context(ExtractSnafu {})?;

    Ok(())
}

#[cfg(windows)]
fn extract_zip(file: &mut std::fs::File, to: &Path) -> Result<()> {
    use zip::ZipArchive;

    let mut archive = ZipArchive::new(file).context(ExtractSnafu {})?;
    archive.extract(to).context(ExtractSnafu {})?;

    Ok(())
}

/// Creates or updates symlinks for a WasmEdge version installation.
///
/// Creates the following symlinks in the base directory:
/// - bin -> versions/<version>/bin
/// - include -> versions/<version>/include
/// - lib -> versions/<version>/lib
///
/// # Arguments
///
/// * `base_dir` - The base WasmEdge installation directory (e.g., ~/.wasmedge)
/// * `version` - The version being installed (e.g., "0.15.0")
///
/// # Errors
///
/// Returns an error if creating or updating symlinks fails.
pub async fn create_version_symlinks(base_dir: &Path, version: &str) -> Result<()> {
    validate_version_component(version)?;
    let normalized_base_dir = crate::commands::normalize_absolute_path(base_dir)?;
    let install_root = open_dir_nofollow(&normalized_base_dir).map_err(|source| Error::Io {
        action: "open install root".to_string(),
        path: normalized_base_dir.display().to_string(),
        source,
    })?;
    let version_path = PathBuf::from("versions").join(version);
    let version_root =
        open_cap_dir_nofollow(&install_root, &version_path).map_err(|source| Error::Io {
            action: "open version directory".to_string(),
            path: normalized_base_dir
                .join(&version_path)
                .display()
                .to_string(),
            source,
        })?;
    if !cap_version_has_runtime(&version_root)? {
        return Err(Error::VersionNotFound {
            version: version.to_string(),
        });
    }
    create_version_symlinks_in(&install_root, &version_root, &normalized_base_dir, version)
}

fn validate_version_component(version: &str) -> Result<()> {
    let mut version_components = Path::new(version).components();
    if !matches!(
        version_components.next(),
        Some(std::path::Component::Normal(_))
    ) || version_components.next().is_some()
    {
        return Err(Error::InvalidPath {
            path: version.to_string(),
            reason: "version must be a single path component".to_string(),
        });
    }
    Ok(())
}

pub(crate) fn create_version_symlinks_in(
    install_root: &Dir,
    version_root: &Dir,
    base_dir: &Path,
    version: &str,
) -> Result<()> {
    validate_version_component(version)?;
    let symlink_dirs = ["bin", "include", "lib", "plugin"];

    // Keep replacement quarantines inside the selected version tree. A crash
    // can therefore never add a new entry directly under the install root; for
    // normal managed versions, removal already deletes that tree recursively.
    //
    // Preflight: refuse *before* mutating anything if any destination is a
    // pre-existing non-symlink entry. `base_dir` is user-controlled (`--path`)
    // and may point at a populated, non-WasmEdge location such as `/usr/local`;
    // replacing `<base_dir>/<dir>` there would delete unrelated files or
    // directories. Scanning up front keeps the refusal atomic: an earlier
    // symlink is never removed or re-pointed before a later foreign entry
    // triggers the error. `symlink_metadata` does not follow links, so only
    // existing symlinks are eligible for replacement.
    for dir in symlink_dirs {
        let symlink_path = base_dir.join(dir);
        match install_root.symlink_metadata(dir) {
            Ok(metadata) if !metadata.file_type().is_symlink() => {
                return InvalidPathSnafu {
                    path: symlink_path.display().to_string(),
                    reason: format!(
                        "refusing to replace existing non-symlink entry `{dir}`; \
                         remove it manually or choose a dedicated install path \
                         (e.g. the default $HOME/.wasmedge install root)"
                    ),
                }
                .fail();
            }
            Ok(_) => {}
            Err(error) if error.kind() == ErrorKind::NotFound => {}
            Err(source) => {
                return Err(Error::Io {
                    action: "inspect version symlink destination".to_string(),
                    path: symlink_path.display().to_string(),
                    source,
                });
            }
        }
    }

    for dir in symlink_dirs {
        let symlink_path = base_dir.join(dir);
        let target_path = PathBuf::from("versions").join(version).join(dir);
        let quarantine = quarantine_entry_in(install_root, dir, version_root).context(IoSnafu {
            path: symlink_path.display().to_string(),
            action: "quarantine old symlink".to_string(),
        })?;

        if let Some(quarantine) = quarantine {
            let metadata = match quarantine.symlink_metadata("entry") {
                Ok(metadata) => metadata,
                Err(source) => {
                    restore_quarantined_entry(quarantine, install_root, dir)?;
                    return Err(Error::Io {
                        action: "inspect quarantined symlink".to_string(),
                        path: symlink_path.display().to_string(),
                        source,
                    });
                }
            };
            if !metadata.file_type().is_symlink() {
                restore_quarantined_entry(quarantine, install_root, dir)?;
                return InvalidPathSnafu {
                    path: symlink_path.display().to_string(),
                    reason: format!(
                        "refusing to replace non-symlink entry `{dir}` that appeared during link creation"
                    ),
                }
                .fail();
            }

            if let Err(source) = create_version_symlink(install_root, &target_path, dir) {
                restore_quarantined_entry(quarantine, install_root, dir)?;
                return Err(Error::Io {
                    action: "create symlink".to_string(),
                    path: symlink_path.display().to_string(),
                    source,
                });
            }

            remove_quarantined_symlink(&quarantine, "entry").context(IoSnafu {
                path: symlink_path.display().to_string(),
                action: "remove old symlink".to_string(),
            })?;
            quarantine.close().context(IoSnafu {
                path: symlink_path.display().to_string(),
                action: "remove old symlink quarantine".to_string(),
            })?;
        } else {
            create_version_symlink(install_root, &target_path, dir).context(IoSnafu {
                path: symlink_path.display().to_string(),
                action: "create symlink".to_string(),
            })?;
        }
        tracing::debug!(symlink = %symlink_path.display(), target = %target_path.display(), "Created symlink");
    }

    Ok(())
}

#[cfg(unix)]
fn create_version_symlink(dir: &Dir, target: &Path, name: &str) -> io::Result<()> {
    dir.symlink(target, name)
}

#[cfg(windows)]
fn create_version_symlink(dir: &Dir, target: &Path, name: &str) -> io::Result<()> {
    dir.symlink_dir(target, name)
}

#[cfg(unix)]
fn remove_quarantined_symlink(dir: &Dir, name: &str) -> io::Result<()> {
    dir.remove_file(name)
}

#[cfg(windows)]
fn remove_quarantined_symlink(dir: &Dir, name: &str) -> io::Result<()> {
    if dir.remove_dir(name).is_err() {
        dir.remove_file(name)?;
    }
    Ok(())
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn create_directory_walk_rejects_parent_symlink_without_writing_through_it() {
        let parent = tempdir().unwrap();
        let outside = tempdir().unwrap();
        let parent = parent.path().canonicalize().unwrap();
        let outside = outside.path().canonicalize().unwrap();
        let redirect = parent.join("redirect");
        std::os::unix::fs::symlink(&outside, &redirect).unwrap();

        let result = open_or_create_dir_nofollow(&redirect.join("install"));

        assert!(
            result.is_err(),
            "a symlinked path component must be rejected"
        );
        assert!(
            !outside.join("install").exists(),
            "directory creation must not follow the symlink into the outside tree"
        );
    }

    #[test]
    fn create_directory_walk_creates_missing_real_components() {
        let parent = tempdir().unwrap();
        let target = parent
            .path()
            .canonicalize()
            .unwrap()
            .join("one/two/install");

        let opened = open_or_create_dir_nofollow(&target).unwrap();

        assert!(target.is_dir());
        assert!(can_write_to_cap_directory(&opened));
    }

    #[test]
    fn runtime_probe_does_not_follow_a_symlinked_bin_directory() {
        let version = tempdir().unwrap();
        let outside = tempdir().unwrap();
        std::fs::write(outside.path().join("wasmedge"), "foreign runtime").unwrap();
        std::os::unix::fs::symlink(outside.path(), version.path().join("bin")).unwrap();
        let version_root = Dir::open_ambient_dir(version.path(), ambient_authority()).unwrap();

        assert!(!cap_version_has_runtime(&version_root).unwrap());
    }

    #[tokio::test]
    async fn capability_copy_rejects_a_symlinked_destination_parent() {
        let source = tempdir().unwrap();
        std::fs::create_dir(source.path().join("bin")).unwrap();
        std::fs::write(source.path().join("bin/wasmedge"), "runtime").unwrap();

        let destination = tempdir().unwrap();
        let outside = tempdir().unwrap();
        std::os::unix::fs::symlink(outside.path(), destination.path().join("bin")).unwrap();
        let destination_root =
            Dir::open_ambient_dir(destination.path(), ambient_authority()).unwrap();

        let result =
            copy_tree_to_cap_dir(source.path(), &destination_root, destination.path()).await;

        assert!(result.is_err());
        assert!(
            !outside.path().join("wasmedge").exists(),
            "copying must not follow a destination parent symlink"
        );
    }

    #[test]
    fn pinned_root_symlinks_ignore_an_ambient_path_replacement() {
        let parent = tempdir().unwrap();
        let install_path = parent.path().join("install");
        let pinned_path = parent.path().join("pinned-install");
        std::fs::create_dir(&install_path).unwrap();
        let install_root = Dir::open_ambient_dir(&install_path, ambient_authority()).unwrap();
        install_root.create_dir_all("versions/0.14.1").unwrap();

        let outside = tempdir().unwrap();
        std::fs::rename(&install_path, &pinned_path).unwrap();
        std::os::unix::fs::symlink(outside.path(), &install_path).unwrap();

        let version_root =
            open_cap_dir_nofollow(&install_root, Path::new("versions/0.14.1")).unwrap();
        create_version_symlinks_in(&install_root, &version_root, &install_path, "0.14.1").unwrap();

        assert!(std::fs::symlink_metadata(pinned_path.join("bin"))
            .unwrap()
            .file_type()
            .is_symlink());
        assert!(
            std::fs::symlink_metadata(outside.path().join("bin")).is_err(),
            "root-link writes must remain anchored to the opened install root"
        );
    }

    /// Regression: a pre-existing broken symlink at the destination must
    /// still be replaced. The previous implementation used `Path::exists()`,
    /// which follows symlinks and reports `false` for a broken link — so the
    /// link wasn't removed and the next `symlink()` call failed with EEXIST.
    #[tokio::test]
    async fn copy_tree_replaces_broken_symlink_in_dest() {
        let from = tempdir().unwrap();
        let to = tempdir().unwrap();

        let real_target = from.path().join("real.txt");
        std::fs::write(&real_target, "hi").unwrap();
        let from_link = from.path().join("link");
        std::os::unix::fs::symlink(&real_target, &from_link).unwrap();

        let bogus_target = to.path().join("does-not-exist");
        let to_link = to.path().join("link");
        std::os::unix::fs::symlink(&bogus_target, &to_link).unwrap();
        assert!(
            !to_link.exists(),
            "test setup: dest symlink should be broken"
        );
        assert!(
            std::fs::symlink_metadata(&to_link).is_ok(),
            "test setup: dest link itself must exist"
        );

        copy_tree(from.path(), to.path())
            .await
            .expect("copy_tree should succeed even with a broken pre-existing symlink");

        let resolved = std::fs::read_link(&to_link).expect("dest link should still be a symlink");
        assert_eq!(
            resolved, real_target,
            "broken symlink should have been replaced with the source link's target"
        );
    }

    /// Regression: a pre-existing *real, non-empty directory* at the
    /// destination must still be replaced when the source has a symlink
    /// at that path. The previous implementation called `remove_file`
    /// unconditionally, which would fail with `EISDIR` and abort the copy.
    #[tokio::test]
    async fn copy_tree_replaces_real_directory_in_dest() {
        let from = tempdir().unwrap();
        let to = tempdir().unwrap();

        let real_target = from.path().join("real.txt");
        std::fs::write(&real_target, "hi").unwrap();
        let from_link = from.path().join("link");
        std::os::unix::fs::symlink(&real_target, &from_link).unwrap();

        let to_link = to.path().join("link");
        std::fs::create_dir(&to_link).unwrap();
        std::fs::write(to_link.join("orphan.txt"), "leftover").unwrap();

        copy_tree(from.path(), to.path())
            .await
            .expect("copy_tree should succeed even when dest is a real non-empty directory");

        let meta = std::fs::symlink_metadata(&to_link).expect("dest entry should still exist");
        assert!(
            meta.file_type().is_symlink(),
            "dest should now be a symlink, not a directory"
        );
        let resolved = std::fs::read_link(&to_link).expect("dest should be a readable symlink");
        assert_eq!(resolved, real_target);
    }

    /// Security regression (unsafe symlink migration): `create_version_symlinks`
    /// must never recursively delete a pre-existing *real* directory at
    /// `<base_dir>/<dir>`. The original implementation called `remove_dir_all`
    /// unconditionally, so `install`/`use --path /usr/local` against a
    /// populated, non-WasmEdge directory would wipe `/usr/local/{bin,include,lib}`.
    /// It must instead refuse with `Error::InvalidPath` and leave the existing
    /// directory and its contents untouched.
    #[tokio::test]
    async fn create_version_symlinks_refuses_to_delete_existing_real_dir() {
        let base = tempdir().unwrap();
        let version = "0.15.0";

        // The freshly-installed versioned payload the symlinks would point at.
        for dir in ["bin", "include", "lib", "plugin"] {
            std::fs::create_dir_all(base.path().join("versions").join(version).join(dir)).unwrap();
        }
        std::fs::write(
            base.path()
                .join("versions")
                .join(version)
                .join("bin/wasmedge"),
            "runtime",
        )
        .unwrap();

        // A pre-existing, foreign real directory (think `/usr/local/bin`) whose
        // contents must survive.
        let preexisting_bin = base.path().join("bin");
        std::fs::create_dir_all(&preexisting_bin).unwrap();
        std::fs::write(preexisting_bin.join("do-not-delete"), "precious").unwrap();

        let result = create_version_symlinks(base.path(), version).await;

        assert!(
            matches!(result, Err(Error::InvalidPath { .. })),
            "expected an InvalidPath refusal, got {result:?}"
        );
        assert!(
            preexisting_bin.join("do-not-delete").exists(),
            "pre-existing directory contents must be preserved, not deleted"
        );
        let meta = std::fs::symlink_metadata(&preexisting_bin).unwrap();
        assert!(
            meta.file_type().is_dir(),
            "pre-existing real directory must remain a real directory, not be replaced by a symlink"
        );
    }

    /// A populated custom root can also contain regular files named after the
    /// managed links. Those files are unrelated user data and must receive the
    /// same all-or-nothing refusal as a real directory.
    #[tokio::test]
    async fn create_version_symlinks_refuses_to_delete_existing_regular_file() {
        let base = tempdir().unwrap();
        let version = "0.15.0";

        for dir in ["bin", "include", "lib", "plugin"] {
            std::fs::create_dir_all(base.path().join("versions").join(version).join(dir)).unwrap();
        }
        std::fs::write(
            base.path()
                .join("versions")
                .join(version)
                .join("bin/wasmedge"),
            "runtime",
        )
        .unwrap();

        let preexisting_lib = base.path().join("lib");
        std::fs::write(&preexisting_lib, "unrelated data").unwrap();

        let result = create_version_symlinks(base.path(), version).await;

        assert!(
            matches!(result, Err(Error::InvalidPath { .. })),
            "expected an InvalidPath refusal, got {result:?}"
        );
        assert_eq!(
            std::fs::read_to_string(&preexisting_lib).unwrap(),
            "unrelated data",
            "pre-existing regular file contents must be preserved"
        );
        assert!(
            std::fs::symlink_metadata(&preexisting_lib)
                .unwrap()
                .is_file(),
            "pre-existing regular file must remain a regular file"
        );
    }

    #[tokio::test]
    async fn create_version_symlinks_replaces_existing_symlinks_without_root_debris() {
        let base = tempdir().unwrap();
        let old_target = tempdir().unwrap();
        let version = "0.15.0";
        let version_root = base.path().join("versions").join(version);

        for dir in ["bin", "include", "lib", "plugin"] {
            std::fs::create_dir_all(version_root.join(dir)).unwrap();
            std::os::unix::fs::symlink(old_target.path(), base.path().join(dir)).unwrap();
        }
        std::fs::write(version_root.join("bin/wasmedge"), "runtime").unwrap();

        create_version_symlinks(base.path(), version).await.unwrap();

        for dir in ["bin", "include", "lib", "plugin"] {
            assert_eq!(
                std::fs::read_link(base.path().join(dir)).unwrap(),
                PathBuf::from("versions").join(version).join(dir)
            );
        }
        let mut version_entries = std::fs::read_dir(&version_root)
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect::<Vec<_>>();
        version_entries.sort();
        assert_eq!(
            version_entries,
            ["bin", "include", "lib", "plugin"].map(std::ffi::OsString::from),
            "successful replacement must remove its internal quarantine directories"
        );
    }

    /// Atomicity: the refusal must not mutate the filesystem at all. The loop
    /// walks `["bin", "include", "lib", "plugin"]` in order, so a real
    /// directory at a *later* entry must not leave an *earlier* symlink already
    /// removed or re-pointed. Here `bin` is a pre-existing symlink (as a
    /// previous install would leave it) and `include` is a foreign real
    /// directory; the call must refuse without touching `bin`.
    #[tokio::test]
    async fn create_version_symlinks_refusal_does_not_mutate_earlier_entries() {
        let base = tempdir().unwrap();
        let version = "0.15.0";

        for dir in ["bin", "include", "lib", "plugin"] {
            std::fs::create_dir_all(base.path().join("versions").join(version).join(dir)).unwrap();
        }
        std::fs::write(
            base.path()
                .join("versions")
                .join(version)
                .join("bin/wasmedge"),
            "runtime",
        )
        .unwrap();

        // `bin` is an existing symlink from a previous install. It points at an
        // absolute target so any re-pointing by the loop (which would use the
        // relative `versions/<version>/bin`) is detectable.
        let old_target = base.path().join("old-bin");
        std::fs::create_dir_all(&old_target).unwrap();
        let bin_link = base.path().join("bin");
        std::os::unix::fs::symlink(&old_target, &bin_link).unwrap();

        // A later entry is a foreign real directory that must trigger refusal.
        let preexisting_include = base.path().join("include");
        std::fs::create_dir_all(&preexisting_include).unwrap();
        std::fs::write(preexisting_include.join("do-not-delete"), "precious").unwrap();

        let result = create_version_symlinks(base.path(), version).await;

        assert!(
            matches!(result, Err(Error::InvalidPath { .. })),
            "expected an InvalidPath refusal, got {result:?}"
        );
        let link_meta = std::fs::symlink_metadata(&bin_link).unwrap();
        assert!(
            link_meta.file_type().is_symlink(),
            "pre-existing `bin` symlink must remain a symlink after the refusal"
        );
        assert_eq!(
            std::fs::read_link(&bin_link).unwrap(),
            old_target,
            "refusal must not re-point an earlier symlink (operation must be atomic)"
        );
        assert!(
            preexisting_include.join("do-not-delete").exists(),
            "pre-existing directory contents must be preserved"
        );
    }
}

#[cfg(test)]
mod temp_workspace_tests {
    use super::*;

    /// A local attacker on a shared temp filesystem can predict the legacy
    /// workspace path (`<base>/<install_name>`) and pre-create it before a
    /// privileged install to redirect download/extract writes (CWE-59 /
    /// CWE-377). Each call must instead yield a fresh, unique directory that
    /// never reuses that predictable path.
    #[test]
    fn temp_workspace_is_unpredictable_and_not_reused() {
        let base = tempfile::tempdir().unwrap();
        let install_name = "WasmEdge-0.14.1-Linux";

        // Simulate an attacker pre-creating the predictable path.
        let predictable = base.path().join(install_name);
        std::fs::create_dir_all(&predictable).unwrap();

        let ws1 = create_temp_workspace(base.path(), install_name).unwrap();
        let ws2 = create_temp_workspace(base.path(), install_name).unwrap();

        assert_ne!(ws1.path(), predictable.as_path());
        assert_ne!(ws2.path(), predictable.as_path());
        assert_ne!(ws1.path(), ws2.path());
        assert!(ws1.path().starts_with(base.path()));
        assert!(ws1.path().is_dir());
        assert_eq!(std::fs::read_dir(ws1.path()).unwrap().count(), 0);
    }

    /// CWE-59 regression: when an attacker pre-creates the legacy predictable
    /// path (`<base>/<install_name>`) as a symlink into a directory they
    /// control, the helper must neither use nor follow it. The defense is that
    /// the workspace is staged under a fresh randomized sibling created with an
    /// exclusive `mkdir`, so the planted symlink is never on the write path.
    /// This test pins that: the returned workspace is a real directory distinct
    /// from the symlink, the symlink is left intact (not followed or clobbered
    /// — a predictable-path regression would do one of those), and an actual
    /// write lands inside `base` rather than leaking through into the attacker's
    /// directory.
    #[cfg(unix)]
    #[test]
    fn temp_workspace_write_is_contained_despite_precreated_symlink() {
        let base = tempfile::tempdir().unwrap();
        let attacker_target = tempfile::tempdir().unwrap();
        let install_name = "WasmEdge-0.14.1-Linux";

        let predictable = base.path().join(install_name);
        std::os::unix::fs::symlink(attacker_target.path(), &predictable).unwrap();

        let ws = create_temp_workspace(base.path(), install_name).unwrap();

        // The helper must avoid the predictable path entirely: a different path
        // AND a still-intact symlink prove it neither adopted nor followed it
        // (the latter would have replaced the symlink with a real directory).
        assert_ne!(ws.path(), predictable.as_path());
        assert!(
            std::fs::symlink_metadata(&predictable)
                .unwrap()
                .file_type()
                .is_symlink(),
            "the planted symlink at the legacy path must be left untouched"
        );

        let staged = ws.path().join("payload");
        std::fs::write(&staged, b"runtime bytes").unwrap();

        let base_canon = base.path().canonicalize().unwrap();
        let attacker_canon = attacker_target.path().canonicalize().unwrap();
        let ws_canon = ws.path().canonicalize().unwrap();

        assert!(ws_canon.starts_with(&base_canon));
        assert!(!ws_canon.starts_with(&attacker_canon));
        assert!(staged.canonicalize().unwrap().starts_with(&base_canon));
        assert_eq!(
            std::fs::read_dir(attacker_target.path()).unwrap().count(),
            0,
            "write leaked through the pre-created symlink into the attacker's directory"
        );
    }

    /// `create_temp_workspace` must create `base` (and any missing parents) on
    /// first use: the real plugin staging root (`<temp>/wasmedgeup/plugins`)
    /// usually does not exist on a fresh machine, so the helper has to
    /// materialize it. A regression dropping the `create_dir_all(base)` step
    /// would still pass the tests above (which pass an already-existing `base`)
    /// yet break the first-ever install with ENOENT from `tempdir_in`.
    #[test]
    fn temp_workspace_creates_missing_base() {
        let root = tempfile::tempdir().unwrap();
        let base = root.path().join("wasmedgeup").join("plugins");
        assert!(!base.exists());

        let ws = create_temp_workspace(&base, "wasi_nn-0.14.1").unwrap();

        assert!(base.is_dir());
        assert!(ws.path().starts_with(&base));
        assert!(ws.path().is_dir());
    }

    /// Plugin names are unvalidated user input (`PluginVersion` only splits on
    /// `@`), and the name is used verbatim as the tempfile prefix that
    /// `tempdir_in` joins onto `base`. A name that is not a single normal path
    /// component would otherwise create the workspace outside the staging root,
    /// so the helper must reject every such escape: a path separator
    /// (`../../evil`), a `.`/`..` element, and — on Windows — a drive-relative
    /// prefix like `C:evil` that `std::path::is_separator` would miss.
    #[test]
    fn temp_workspace_rejects_non_component_names() {
        let base = tempfile::tempdir().unwrap();

        for bad in ["../../evil", "..", ".", "a/b", "trailing/"] {
            let err = create_temp_workspace(base.path(), bad).unwrap_err();
            assert!(
                matches!(err, Error::InvalidPath { .. }),
                "expected InvalidPath for {bad:?}, got {err:?}"
            );
        }

        // A drive-relative prefix has no separator but still escapes `base` on
        // Windows (`base.join("C:evil-...")` discards `base`); it must be
        // rejected there. On Unix `:` is an ordinary filename character, so the
        // same string is a legitimate single component and stays accepted.
        #[cfg(windows)]
        {
            let err = create_temp_workspace(base.path(), "C:evil").unwrap_err();
            assert!(
                matches!(err, Error::InvalidPath { .. }),
                "expected InvalidPath for a drive-relative name, got {err:?}"
            );
        }

        // A normal single-component name is still accepted.
        assert!(create_temp_workspace(base.path(), "wasi_nn-0.14.1").is_ok());
    }
}

#[cfg(all(test, windows))]
mod windows_tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn handle_relative_rename_is_no_replace() {
        let root = tempdir().unwrap();
        let from_path = root.path().join("from");
        let to_path = root.path().join("to");
        std::fs::create_dir(&from_path).unwrap();
        std::fs::create_dir(&to_path).unwrap();
        std::fs::create_dir(from_path.join("entry")).unwrap();
        std::fs::write(from_path.join("entry/marker"), "source").unwrap();

        let from_dir = Dir::open_ambient_dir(&from_path, ambient_authority()).unwrap();
        let to_dir = Dir::open_ambient_dir(&to_path, ambient_authority()).unwrap();
        rename_noreplace(
            &from_dir,
            Path::new("entry"),
            &to_dir,
            Path::new("published"),
        )
        .unwrap();
        assert_eq!(
            std::fs::read_to_string(to_path.join("published/marker")).unwrap(),
            "source"
        );

        std::fs::create_dir(from_path.join("entry")).unwrap();
        std::fs::write(from_path.join("entry/marker"), "second source").unwrap();
        let result = rename_noreplace(
            &from_dir,
            Path::new("entry"),
            &to_dir,
            Path::new("published"),
        );

        assert!(
            result.is_err(),
            "an existing destination must not be replaced"
        );
        assert_eq!(
            std::fs::read_to_string(from_path.join("entry/marker")).unwrap(),
            "second source"
        );
        assert_eq!(
            std::fs::read_to_string(to_path.join("published/marker")).unwrap(),
            "source"
        );
    }
}

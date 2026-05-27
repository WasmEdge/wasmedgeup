use crate::prelude::*;
use snafu::ResultExt;

use std::io::Seek;

#[cfg(unix)]
use std::os::unix::fs::symlink as symlink_unix;

use std::path::Path;

#[cfg(windows)]
use std::os::windows::fs::{symlink_dir, symlink_file};

use std::fs::OpenOptions;
use tempfile::{Builder, TempDir};
use tokio::fs;
use walkdir::WalkDir;

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
    let symlink_dirs = ["bin", "include", "lib", "plugin"];

    // Preflight: refuse *before* mutating anything if any destination is a
    // pre-existing real directory. `base_dir` is user-controlled (`--path`) and
    // may point at a populated, non-WasmEdge location such as `/usr/local`;
    // recursively removing `<base_dir>/<dir>` there would wipe directories like
    // `/usr/local/bin`. Scanning up front keeps the refusal atomic: an earlier
    // entry's symlink is never removed or re-pointed before a later real
    // directory triggers the error. `symlink_metadata` does not follow links,
    // so existing symlinks/files fall through to the mutation loop below.
    for dir in symlink_dirs {
        let symlink_path = base_dir.join(dir);
        if let Ok(meta) = fs::symlink_metadata(&symlink_path).await {
            if meta.file_type().is_dir() {
                return InvalidPathSnafu {
                    path: symlink_path.display().to_string(),
                    reason: format!(
                        "refusing to replace existing directory `{dir}` with a symlink; \
                         remove it manually or choose a dedicated install path \
                         (e.g. the default $HOME/.wasmedge install root)"
                    ),
                }
                .fail();
            }
        }
    }

    for dir in symlink_dirs {
        let symlink_path = base_dir.join(dir);

        #[cfg(unix)]
        let target_path = format!("versions/{version}/{dir}");
        #[cfg(windows)]
        let target_path = base_dir.join("versions").join(version).join(dir);

        if let Ok(meta) = fs::symlink_metadata(&symlink_path).await {
            let file_type = meta.file_type();

            #[cfg(windows)]
            {
                use tokio::fs::{remove_dir, remove_file};

                // A pre-existing real directory was already rejected by the
                // preflight scan above, so the destination is a symlink or file.
                if file_type.is_symlink() {
                    match remove_dir(&symlink_path).await {
                        Ok(_) => {}
                        Err(_) => {
                            remove_file(&symlink_path).await.context(IoSnafu {
                                path: symlink_path.display().to_string(),
                                action: "remove old symlink".to_string(),
                            })?;
                        }
                    }
                } else {
                    remove_file(&symlink_path).await.context(IoSnafu {
                        path: symlink_path.display().to_string(),
                        action: "remove existing file before creating symlink".to_string(),
                    })?;
                }
            }

            #[cfg(unix)]
            {
                // A pre-existing real directory was already rejected by the
                // preflight scan above, so only replace symlinks or files.
                if file_type.is_symlink() || file_type.is_file() {
                    fs::remove_file(&symlink_path).await.context(IoSnafu {
                        path: symlink_path.display().to_string(),
                        action: "remove old symlink".to_string(),
                    })?;
                }
            }
        }

        #[cfg(unix)]
        {
            symlink_unix(&target_path, &symlink_path).context(IoSnafu {
                path: symlink_path.display().to_string(),
                action: "create symlink".to_string(),
            })?;
        }
        #[cfg(windows)]
        {
            symlink_dir(&target_path, &symlink_path).context(IoSnafu {
                path: symlink_path.display().to_string(),
                action: "create symlink".to_string(),
            })?;
        }
        #[cfg(unix)]
        tracing::debug!(symlink = %symlink_path.display(), target = %target_path, "Created symlink");
        #[cfg(windows)]
        tracing::debug!(symlink = %symlink_path.display(), target = %target_path.display(), "Created symlink");
    }

    Ok(())
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use tempfile::tempdir;

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

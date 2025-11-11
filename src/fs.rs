use crate::prelude::*;
use snafu::ResultExt;

use std::io::Seek;

#[cfg(unix)]
use std::os::unix::fs::symlink as symlink_unix;

use std::path::Path;

#[cfg(windows)]
use std::os::windows::fs::{symlink_dir, symlink_file};
#[cfg(windows)]
use std::path::Component;

use std::fs::OpenOptions;
use tokio::fs;
use walkdir::WalkDir;

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

pub async fn copy_tree(from_dir: &Path, to_dir: &Path) -> Result<()> {
    for entry in WalkDir::new(from_dir).into_iter().filter_map(|e| e.ok()) {
        tracing::trace!(entry = %entry.path().display(), "Copying entry");
        let Ok(metadata) = entry.metadata() else {
            continue;
        };
        if !metadata.is_file() && !metadata.is_symlink() {
            continue;
        }

        // Calculate the target location by stripping the source directory prefix
        // from the entry path and appending it to the destination directory.
        // During this process, any 'lib64' directory is renamed to 'lib' for consistency.
        //
        // # Example
        // from_dir = '/from/path'
        // entry = '/from/path/foo/lib64/something.so'
        // to_dir = '/to/path'
        //
        // 1. Strip prefix: 'foo/lib64/something.so'
        // 2. Replace lib64: 'foo/lib/something.so'
        // 3. Join with to_dir: '/to/path/foo/lib/something.so'
        let target_loc = to_dir.join(
            entry
                .path()
                .strip_prefix(from_dir)
                .unwrap_or(entry.path())
                .to_string_lossy()
                .replace("lib64", LIB_DIR),
        );

        let Some(parent) = target_loc.parent() else {
            tracing::warn!(location = %target_loc.display(), "Missing parent for target location");
            continue;
        };
        if let Err(e) = fs::create_dir_all(parent).await {
            tracing::warn!(error = %e, directories = %parent.display(), "Failed to create directories");
            continue;
        };
        if metadata.is_symlink() {
            if let Ok(target) = std::fs::read_link(entry.path()) {
                if target_loc.exists() {
                    match fs::remove_file(&target_loc).await {
                        Ok(_) => {}
                        Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => {
                            #[cfg(windows)]
                            return Err(Error::WindowsSymlinkError {
                                version: std::env::var("WASMEDGE_VERSION")
                                    .unwrap_or_else(|_| "latest".to_string()),
                            });

                            #[cfg(not(windows))]
                            tracing::warn!(
                                error = %e,
                                path = %target_loc.display(),
                                "Failed to remove existing symlink due to permissions"
                            );
                        }
                        Err(e) => {
                            tracing::warn!(
                                error = %e,
                                path = %target_loc.display(),
                                "Failed to remove existing symlink"
                            );
                        }
                    }
                }

                #[cfg(unix)]
                {
                    if let Err(e) = symlink_unix(&target, &target_loc) {
                        tracing::warn!(
                            error = %e,
                            entry = %entry.path().display(),
                            target_loc = %target_loc.display(),
                            "Failed to create symlink"
                        );
                    }
                }

                #[cfg(windows)]
                {
                    let is_dir = std::fs::metadata(entry.path())
                        .map(|m| m.is_dir())
                        .unwrap_or(false);

                    if target_loc.exists() {
                        let remove_result = if is_dir {
                            fs::remove_dir(&target_loc).await
                        } else {
                            fs::remove_file(&target_loc).await
                        };
                        if let Err(e) = remove_result {
                            tracing::warn!(
                                error = %e,
                                path = %target_loc.display(),
                                "Failed to remove existing symlink"
                            );
                            continue;
                        }
                    }

                    let res = if is_dir {
                        symlink_dir(&target, &target_loc)
                    } else {
                        symlink_file(&target, &target_loc)
                    };
                    if let Err(e) = res {
                        if e.kind() == std::io::ErrorKind::PermissionDenied {
                            #[cfg(windows)]
                            return Err(Error::WindowsSymlinkError {
                                version: std::env::var("WASMEDGE_VERSION")
                                    .unwrap_or_else(|_| "latest".to_string()),
                            });
                        }
                        tracing::warn!(
                            error = %e,
                            entry = %entry.path().display(),
                            target_loc = %target_loc.display(),
                            "Failed to create symlink (Windows)"
                        );
                    }
                }
            }
        } else if let Err(e) = fs::copy(entry.path(), &target_loc).await {
            tracing::warn!(
                error = %e,
                entry = %entry.path().display(),
                target_loc = %target_loc.display(),
                "Failed to copy file to target location",
            );
        };
    }
    Ok(())
}

/// Extracts the contents of a compressed archive (`.tar.gz` for Unix-like systems, `.zip` for Windows) to a specified directory.
///
/// # Arguments
///
/// * `file` - A file object representing the compressed archive. This file must be opened in read mode.
/// * `dest` - The destination directory to which the contents will be extracted.
///
/// # Errors
///
/// Returns an error if the extraction fails. This could happen if the archive format is unsupported or
/// if the destination path cannot be created.
pub async fn extract_archive(file: &mut std::fs::File, dest: &Path) -> Result<()> {
    fs::create_dir_all(dest).await.inspect_err(
        |e| tracing::error!(error = %e.to_string(), "Failed to create directory during extraction"),
    )?;
    file.rewind()?;

    #[cfg(unix)]
    {
        use flate2::read::GzDecoder;
        let decompressed = GzDecoder::new(file);
        extract_tar(decompressed, dest)?;
    }

    #[cfg(windows)]
    extract_zip(file, dest)?;

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
                use tokio::fs::{remove_dir, remove_dir_all, remove_file};

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
                } else if file_type.is_dir() {
                    remove_dir_all(&symlink_path).await.context(IoSnafu {
                        path: symlink_path.display().to_string(),
                        action: "remove existing directory before creating symlink".to_string(),
                    })?;
                } else {
                    remove_file(&symlink_path).await.context(IoSnafu {
                        path: symlink_path.display().to_string(),
                        action: "remove existing file before creating symlink".to_string(),
                    })?;
                }
            }

            #[cfg(unix)]
            {
                if file_type.is_symlink() || file_type.is_file() {
                    fs::remove_file(&symlink_path).await.context(IoSnafu {
                        path: symlink_path.display().to_string(),
                        action: "remove old symlink".to_string(),
                    })?;
                } else if file_type.is_dir() {
                    fs::remove_dir_all(&symlink_path).await.context(IoSnafu {
                        path: symlink_path.display().to_string(),
                        action: "remove existing directory before creating symlink".to_string(),
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

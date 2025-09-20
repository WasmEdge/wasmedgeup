use crate::prelude::*;
use snafu::ResultExt;

use std::io::Seek;

#[cfg(unix)]
use std::os::unix::fs::symlink as symlink_unix;

#[cfg(windows)]
use std::os::windows::fs::{symlink_dir, symlink_file};

use std::path::{Path, PathBuf};

use tokio::fs;
use walkdir::WalkDir;

pub async fn copy_tree(from_dir: &Path, to_dir: &Path) {
    let num_components = from_dir.components().count();

    for entry in WalkDir::new(from_dir).into_iter().filter_map(|e| e.ok()) {
        tracing::trace!(entry = %entry.path().display(), "Copying entry");
        let Ok(metadata) = entry.metadata() else {
            continue;
        };
        if !metadata.is_file() && !metadata.is_symlink() {
            continue;
        }

        // Calculate the target location based on from_dir, to_dir, and entry
        // by first calculate the path of entry relative to from_dir, and then append it to to_dir
        //
        // # Example
        // from_dir = '/from/path
        // entry = '/from/path/foo/bar/something.txt'
        // to_dir = '/to/path'
        // => num_components = 3 ([RootDir, "from", "path"])
        // => chained = [RootDir, "to", "path"].chain(["foo", "bar", "something.txt"])
        // => target_loc = "/to/path/foo/bar/something.txt"
        let target_loc = to_dir
            .components()
            .chain(entry.path().components().skip(num_components))
            .collect::<PathBuf>();

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
                    let res = if is_dir {
                        symlink_dir(&target, &target_loc)
                    } else {
                        symlink_file(&target, &target_loc)
                    };
                    if let Err(e) = res {
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

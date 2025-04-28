use crate::prelude::*;
use snafu::ResultExt;

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
        if !metadata.is_file() {
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

        if let Err(e) = fs::copy(entry.path(), &target_loc).await {
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
///
/// # Example
/// ```rust
/// let file = std::fs::File::open("archive.tar.gz")?;
/// let dest = Path::new("/path/to/destination");
/// extract_archive(file, dest).await?;
/// ```
pub async fn extract_archive(mut file: std::fs::File, dest: &Path) -> Result<()> {
    use std::io::Seek;
    use tokio::fs;

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
fn extract_zip(file: std::fs::File, to: &Path) -> Result<()> {
    use zip::ZipArchive;

    let mut archive = ZipArchive::new(file).context(ExtractSnafu {})?;
    archive.extract(to).context(ExtractSnafu {})?;

    Ok(())
}

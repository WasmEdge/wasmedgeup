use std::{
    io::{ErrorKind, Read},
    path::Path,
};

use cap_primitives::fs::FollowSymlinks;
use cap_std::fs::{Dir, OpenOptions};
use semver::Version;

use crate::{
    constants::{VERSION_INSTALL_MARKER, VERSION_INSTALL_MARKER_CONTENT},
    fs::{cap_version_has_runtime, open_cap_dir_nofollow, open_dir_nofollow},
    prelude::*,
};

pub(crate) fn open_install_root(path: &Path) -> Result<Option<Dir>> {
    match open_dir_nofollow(path) {
        Ok(root) => Ok(Some(root)),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(None),
        Err(error) => Err(Error::InvalidPath {
            path: path.display().to_string(),
            reason: format!(
                "the install path must be a real directory opened without following symlinks: {error}"
            ),
        }),
    }
}

pub(crate) fn open_versions_root(install_root: &Dir, install_path: &Path) -> Result<Option<Dir>> {
    let versions_path = install_path.join("versions");
    let metadata = match install_root.symlink_metadata("versions") {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(Error::InvalidPath {
            path: versions_path.display().to_string(),
            reason: "the versions path must be a real directory, not a symlink or file".to_string(),
        });
    }

    open_cap_dir_nofollow(install_root, Path::new("versions"))
        .map(Some)
        .map_err(|error| Error::InvalidPath {
            path: versions_path.display().to_string(),
            reason: format!("the versions path must be opened without following symlinks: {error}"),
        })
}

pub(crate) fn managed_versions(versions_root: &Dir) -> Result<Vec<Version>> {
    let mut versions = Vec::new();

    for entry in versions_root.entries()? {
        let entry = entry?;
        let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
            continue;
        };
        let Ok(version) = Version::parse(&name) else {
            continue;
        };

        if entry.file_type()?.is_dir()
            && open_managed_version_dir(versions_root, Path::new(&name))?.is_some()
        {
            versions.push(version);
        } else {
            tracing::debug!(version = %name, "Preserving unowned semantic-version entry");
        }
    }

    Ok(versions)
}

pub(crate) fn usable_managed_versions(versions_root: &Dir) -> Result<Vec<Version>> {
    let mut versions = Vec::new();
    for version in managed_versions(versions_root)? {
        if managed_version_is_usable(versions_root, &version)? {
            versions.push(version);
        }
    }
    Ok(versions)
}

pub(crate) fn latest_usable_managed_version(versions_root: &Dir) -> Result<Option<Version>> {
    Ok(usable_managed_versions(versions_root)?.into_iter().max())
}

pub(crate) fn select_usable_managed_runtime(
    versions_root: &Dir,
    requested: Option<&str>,
    empty_version_label: &str,
) -> Result<(Version, Dir)> {
    let version = if let Some(requested) = requested {
        Version::parse(requested).map_err(|source| Error::SemVer { source })?
    } else {
        latest_usable_managed_version(versions_root)?.ok_or_else(|| Error::VersionNotFound {
            version: empty_version_label.to_string(),
        })?
    };

    let version_root =
        open_usable_managed_version_dir(versions_root, &version)?.ok_or_else(|| {
            Error::VersionNotFound {
                version: version.to_string(),
            }
        })?;
    Ok((version, version_root))
}

pub(crate) fn managed_version_is_usable(versions_root: &Dir, version: &Version) -> Result<bool> {
    Ok(open_usable_managed_version_dir(versions_root, version)?.is_some())
}

pub(crate) fn open_usable_managed_version_dir(
    versions_root: &Dir,
    version: &Version,
) -> Result<Option<Dir>> {
    let name = version.to_string();
    let Some(version_root) = open_managed_version_dir(versions_root, Path::new(&name))? else {
        return Ok(None);
    };
    if cap_version_has_runtime(&version_root)? {
        Ok(Some(version_root))
    } else {
        Ok(None)
    }
}

pub(crate) fn open_managed_version_dir(versions_root: &Dir, name: &Path) -> Result<Option<Dir>> {
    let metadata = match versions_root.symlink_metadata(name) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Ok(None);
    }

    let version_root = open_cap_dir_nofollow(versions_root, name)?;
    if cap_file_matches(
        &version_root,
        VERSION_INSTALL_MARKER,
        VERSION_INSTALL_MARKER_CONTENT,
    )? || cap_version_has_runtime(&version_root)?
    {
        return Ok(Some(version_root));
    }

    Ok(None)
}

pub(crate) fn cap_file_matches(dir: &Dir, name: &str, expected: &str) -> std::io::Result<bool> {
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

    let mut options = OpenOptions::new();
    options.read(true)._cap_fs_ext_follow(FollowSymlinks::No);
    let file = match dir.open_with(name, &options) {
        Ok(file) => file,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(error),
    };
    let metadata = file.metadata()?;
    if !metadata.is_file() || metadata.len() != expected.len() as u64 {
        return Ok(false);
    }

    let mut content = Vec::with_capacity(expected.len() + 1);
    file.take(expected.len() as u64 + 1)
        .read_to_end(&mut content)?;
    Ok(content == expected.as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;
    use cap_std::ambient_authority;

    fn create_runtime(versions: &Path, version: &str) {
        let bin = versions.join(version).join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        std::fs::write(bin.join("wasmedge"), "runtime").unwrap();
    }

    #[test]
    fn discovery_filters_unowned_and_unusable_version_directories() {
        let root = tempfile::tempdir().unwrap();
        let versions = root.path().join("versions");
        std::fs::create_dir(&versions).unwrap();
        create_runtime(&versions, "0.14.1");

        let partial = versions.join("0.15.0");
        std::fs::create_dir(&partial).unwrap();
        std::fs::write(
            partial.join(VERSION_INSTALL_MARKER),
            VERSION_INSTALL_MARKER_CONTENT,
        )
        .unwrap();

        std::fs::create_dir(versions.join("0.16.0")).unwrap();
        std::fs::create_dir(versions.join("backups")).unwrap();

        let versions_root = Dir::open_ambient_dir(&versions, ambient_authority()).unwrap();
        let mut managed = managed_versions(&versions_root).unwrap();
        managed.sort();
        assert_eq!(
            managed,
            [
                Version::parse("0.14.1").unwrap(),
                Version::parse("0.15.0").unwrap()
            ]
        );
        assert_eq!(
            usable_managed_versions(&versions_root).unwrap(),
            [Version::parse("0.14.1").unwrap()]
        );
        assert_eq!(
            latest_usable_managed_version(&versions_root).unwrap(),
            Some(Version::parse("0.14.1").unwrap())
        );
    }

    #[test]
    fn explicit_selection_rejects_unowned_and_marker_only_versions() {
        let root = tempfile::tempdir().unwrap();
        let versions = root.path().join("versions");
        std::fs::create_dir(&versions).unwrap();
        create_runtime(&versions, "0.14.1");
        std::fs::create_dir(versions.join("0.15.0")).unwrap();
        let partial = versions.join("0.16.0");
        std::fs::create_dir(&partial).unwrap();
        std::fs::write(
            partial.join(VERSION_INSTALL_MARKER),
            VERSION_INSTALL_MARKER_CONTENT,
        )
        .unwrap();

        let versions_root = Dir::open_ambient_dir(&versions, ambient_authority()).unwrap();
        assert!(select_usable_managed_runtime(&versions_root, Some("0.14.1"), "latest").is_ok());
        for version in ["0.15.0", "0.16.0"] {
            assert!(matches!(
                select_usable_managed_runtime(&versions_root, Some(version), "latest"),
                Err(Error::VersionNotFound { .. })
            ));
        }
    }
}

use crate::prelude::*;
use std::{
    io::{self, ErrorKind},
    path::{Path, PathBuf},
};

pub mod install;
pub mod list;
pub mod plugin;
pub mod remove;
pub mod use_cmd;

fn default_path() -> Result<PathBuf> {
    let home_dir = dirs::home_dir().ok_or(Error::HomeDirNotFound)?;
    Ok(home_dir.join(".wasmedge"))
}

/// Resolve the WasmEdge install root: an explicit `--path` wins, otherwise
/// fall back to `$HOME/.wasmedge`.
pub fn resolve_install_path(path: Option<PathBuf>) -> Result<PathBuf> {
    match path {
        Some(p) => Ok(p),
        None => default_path(),
    }
}

pub(crate) fn normalize_absolute_path(path: &Path) -> io::Result<PathBuf> {
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

pub fn insufficient_permissions(path: &Path, action: &str, version: &str) -> Error {
    let system_dir = if cfg!(windows) {
        "C:\\Program Files\\WasmEdge".to_string()
    } else {
        "/usr/local".to_string()
    };
    let sudo = if cfg!(windows) {
        "".to_string()
    } else {
        "sudo ".to_string()
    };

    Error::InsufficientPermissions {
        path: path.display().to_string(),
        action: action.to_string(),
        version: version.to_string(),
        system_dir,
        sudo,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_install_path_prefers_explicit_path() {
        let explicit = PathBuf::from("/opt/custom-wasmedge");
        let resolved = resolve_install_path(Some(explicit.clone())).expect("explicit path");
        assert_eq!(resolved, explicit);
    }

    #[test]
    fn resolve_install_path_falls_back_to_default() {
        let resolved = resolve_install_path(None).expect("default path");
        let expected = dirs::home_dir().expect("home dir").join(".wasmedge");
        assert_eq!(resolved, expected);
    }

    #[test]
    fn normalize_absolute_path_collapses_equivalent_relative_spellings() {
        assert_eq!(
            normalize_absolute_path(Path::new("relative-install")).unwrap(),
            normalize_absolute_path(Path::new("./relative-install")).unwrap()
        );
    }
}

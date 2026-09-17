use crate::prelude::*;
use std::path::{Path, PathBuf};

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
}

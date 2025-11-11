use crate::error::Error;
use std::path::{Path, PathBuf};

pub mod install;
pub mod list;
pub mod plugin;
pub mod remove;
pub mod use_cmd;

fn default_path() -> PathBuf {
    let home_dir = dirs::home_dir().expect("home_dir should be present");
    home_dir.join(".wasmedge")
}

pub fn insufficient_permissions(path: &Path, action: &str, version: &str) -> Error {
    let system_dir = if cfg!(windows) {
        "C\\Program Files\\WasmEdge".to_string()
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

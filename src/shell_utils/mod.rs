#[cfg(unix)]
mod unix;
#[cfg(all(test, unix))]
pub(crate) use unix::managed_shell_script_content;
#[cfg(unix)]
pub(crate) use unix::remove_managed_shell_scripts;
#[cfg(unix)]
pub(crate) use unix::setup_path_in;
#[cfg(unix)]
pub(crate) use unix::validate_shell_configuration_path;
#[cfg(unix)]
pub use unix::{get_available_shells, setup_path, uninstall_path, uninstall_path_configuration};

#[cfg(windows)]
mod windows;
#[cfg(windows)]
pub(crate) use windows::remove_managed_shell_scripts;
#[cfg(windows)]
pub(crate) use windows::setup_path_in;
#[cfg(windows)]
pub(crate) use windows::validate_shell_configuration_path;
#[cfg(windows)]
pub use windows::{setup_path, uninstall_path, uninstall_path_configuration};

use crate::prelude::Result;
use std::path::PathBuf;

// Recovered ownership paths must remain available even when a later script
// operation fails, so callers can remove every known shell configuration line.
pub(crate) struct ManagedShellScriptCleanup {
    pub(crate) configured_paths: Vec<PathBuf>,
    pub(crate) result: Result<()>,
}

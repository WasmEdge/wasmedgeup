#[cfg(unix)]
mod unix;
#[cfg(unix)]
pub(crate) use unix::managed_shell_script_content;
#[cfg(unix)]
pub use unix::{get_available_shells, setup_path, uninstall_path, uninstall_path_configuration};

#[cfg(windows)]
mod windows;
#[cfg(windows)]
pub use windows::{setup_path, uninstall_path, uninstall_path_configuration};

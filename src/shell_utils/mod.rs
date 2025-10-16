#[cfg(unix)]
mod unix;
#[cfg(unix)]
pub use unix::{get_available_shells, setup_path, uninstall_path};

#[cfg(windows)]
mod windows;
#[cfg(windows)]
pub use windows::{setup_path, uninstall_path};

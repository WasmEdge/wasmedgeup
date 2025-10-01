use std::path::PathBuf;

pub mod install;
pub mod list;
pub mod plugin;
pub mod remove;
pub mod use_cmd;

fn default_path() -> PathBuf {
    let home_dir = dirs::home_dir().expect("home_dir should be present");
    home_dir.join(".wasmedge")
}

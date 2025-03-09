use std::path::PathBuf;

use clap::Parser;

use crate::target::{TargetArch, TargetOS};

#[derive(Debug, Parser)]
pub struct InstallArgs {
    /// WasmEdge version to install, e.g. `latest`, `0.14.1`, `0.14.1-rc.1`, etc.
    pub version: String,

    /// Set the install location for the WasmEdge runtime
    ///
    /// Defaults to `$HOME/.wasmedge` on Unix-like systems and `%HOME%\.wasmedge` on Windows.
    #[arg(short, long)]
    pub path: Option<PathBuf>,

    /// Set the temporary directory for staging downloaded assets
    ///
    /// Defaults to `/tmp` on Unix-like systems and `%LOCALAPPDATA%\Temp` on Windows.
    #[arg(short, long)]
    pub tmpdir: Option<PathBuf>,

    /// Set the target OS for the WasmEdge runtime
    ///
    /// `wasmedgeup` will detect the OS of your host system by default.
    #[arg(short, long)]
    pub os: Option<TargetOS>,

    /// Set the target architecture for the WasmEdge runtime
    ///
    /// `wasmedgeup` will detect the architecture of your host system by default.
    #[arg(short, long)]
    pub arch: Option<TargetArch>,
}

use clap::Parser;

use crate::target::{TargetArch, TargetOS};

#[derive(Debug, Parser)]
pub struct RemoveArgs {
    /// WasmEdge version to remove, e.g. `latest`, `0.14.1`, `0.14.1-rc.1`, etc.
    pub version: String,

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

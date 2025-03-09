mod install;
mod remove;
mod version;

use clap::{Parser, Subcommand};
use install::PluginInstallArgs;
use remove::PluginRemoveArgs;

#[derive(Debug, Parser)]
pub struct PluginCli {
    #[command(subcommand)]
    commands: PluginCommands,
}

#[derive(Debug, Subcommand)]
pub enum PluginCommands {
    /// Install the specified WasmEdge plugin(s)
    Install(PluginInstallArgs),
    /// List all available WasmEdge plugins according to the installed WasmEdge runtime version
    List,
    /// Uninstall the specified WasmEdge plugin(s)
    Remove(PluginRemoveArgs),
}

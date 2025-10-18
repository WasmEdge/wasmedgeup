mod install;
mod remove;
mod specs;
mod version;

use crate::cli::{CommandContext, CommandExecutor};
use crate::prelude::*;
use clap::{Parser, Subcommand};
use install::PluginInstallArgs;
use remove::PluginRemoveArgs;
use specs::PluginSpecsArgs;

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
    /// Show detected system specs used for plugin compatibility and selection
    Specs(PluginSpecsArgs),
}

impl CommandExecutor for PluginCli {
    async fn execute(self, ctx: CommandContext) -> Result<()> {
        match self.commands {
            PluginCommands::Specs(args) => args.execute(ctx).await,
            _ => Err(Error::Unknown),
        }
    }
}

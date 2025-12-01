pub mod install;
pub mod list;
pub mod remove;
pub mod utils;
pub mod version;

use crate::cli::{CommandContext, CommandExecutor};
use crate::prelude::*;
use clap::{Parser, Subcommand};
use install::PluginInstallArgs;
use list::PluginListArgs;
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
    /// List WasmEdge plugins available for the current runtime/platform (or all with --all)
    List(PluginListArgs),
    /// Uninstall the specified WasmEdge plugin(s)
    Remove(PluginRemoveArgs),
}

impl CommandExecutor for PluginCli {
    async fn execute(self, ctx: CommandContext) -> Result<()> {
        match self.commands {
            PluginCommands::Install(args) => args.execute(ctx).await,
            PluginCommands::List(args) => args.execute(ctx).await,
            PluginCommands::Remove(args) => args.execute(ctx).await,
        }
    }
}

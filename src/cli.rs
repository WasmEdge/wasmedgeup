use std::future::Future;

use crate::api::WasmEdgeApiClient;
use crate::commands::install::InstallArgs;
use crate::commands::list::ListArgs;
use crate::commands::plugin::PluginCli;
use crate::commands::remove::RemoveArgs;
use crate::prelude::*;
use clap::builder::styling::AnsiColor;
use clap::{builder::Styles, Parser, Subcommand};

const STYLES: Styles = Styles::styled()
    .header(AnsiColor::Yellow.on_default())
    .usage(AnsiColor::Green.on_default())
    .literal(AnsiColor::Green.on_default())
    .placeholder(AnsiColor::Green.on_default());

#[derive(Debug, Parser)]
#[command(name = "wasmedgeup", version = env!("CARGO_PKG_VERSION"))]
#[command(
    about = "WasmEdge runtime installer capable of OS/architectures detection and plugins management"
)]
#[command(arg_required_else_help = true)]
#[command(styles = STYLES)]
pub struct Cli {
    /// Enable verbose output
    #[arg(short, long, action = clap::ArgAction::Count, conflicts_with = "quiet")]
    pub verbose: u8,

    /// Disable progress output
    #[arg(short, long, conflicts_with = "verbose")]
    pub quiet: bool,

    #[command(subcommand)]
    pub commands: Option<Commands>,
}

#[derive(Debug, Clone, Default)]
pub struct CommandContext {
    pub client: WasmEdgeApiClient,
    pub no_progress: bool,
}

impl Cli {
    pub fn context(&self, client: WasmEdgeApiClient) -> CommandContext {
        CommandContext {
            client,
            no_progress: self.quiet,
        }
    }
}

pub trait CommandExecutor {
    fn execute(self, ctx: CommandContext) -> impl Future<Output = Result<()>> + Send;
}

#[derive(Debug, Subcommand)]
pub enum Commands {
    /// Install a specified WasmEdge runtime version
    Install(InstallArgs),
    /// Lists available WasmEdge releases.
    /// By default, only stable releases are shown.
    List(ListArgs),
    /// Uninstall a specific version of WasmEdge from the system
    Remove(RemoveArgs),
    /// Manage WasmEdge plugins
    Plugin(PluginCli),
}

impl CommandExecutor for Commands {
    async fn execute(self, ctx: CommandContext) -> Result<()> {
        use Commands::*;

        match self {
            List(args) => args.execute(ctx).await,
            Install(args) => args.execute(ctx).await,
            _ => todo!(),
        }
    }
}

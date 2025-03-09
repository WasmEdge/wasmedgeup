use crate::commands::install::InstallArgs;
use crate::commands::plugin::PluginCli;
use crate::commands::remove::RemoveArgs;
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

#[derive(Debug, Subcommand)]
pub enum Commands {
    /// Install a specified WasmEdge runtime version
    Install(InstallArgs),
    /// List available WasmEdge releases
    List,
    /// Uninstall a specific version of WasmEdge from the system
    Remove(RemoveArgs),
    /// Manage WasmEdge plugins
    Plugin(PluginCli),
}

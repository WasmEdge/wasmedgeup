use clap::{Arg, ArgMatches, Command};
use anyhow::Result;

mod commands;
mod platform;
mod error;
mod config;

use commands::{install, list, remove, plugin};
use error::WasmedgeupError;

#[tokio::main]
async fn main() -> Result<()> {
    let matches = build_cli().get_matches();

    let result = match matches.subcommand() {
        Some(("install", sub_matches)) => {
            install::execute(sub_matches).await
        }
        Some(("list", sub_matches)) => {
            list::execute(sub_matches).await
        }
        Some(("remove", sub_matches)) => {
            remove::execute(sub_matches).await
        }
        Some(("plugin", sub_matches)) => {
            plugin::execute(sub_matches).await
        }
        _ => {
            println!("Use --help for usage information");
            Ok(())
        }
    };

    if let Err(e) = result {
        eprintln!("Error: {}", e);
        std::process::exit(1);
    }

    Ok(())
}

fn build_cli() -> Command {
    Command::new("wasmedgeup")
        .about("WasmEdge runtime and plugin manager")
        .version(env!("CARGO_PKG_VERSION"))
        .subcommand_required(false)
        .arg_required_else_help(true)
        
        // Global options
        .arg(Arg::new("verbose")
            .short('V')
            .long("verbose")
            .action(clap::ArgAction::SetTrue)
            .help("Enable verbose output")
            .global(true))
            
        .arg(Arg::new("quiet")
            .short('q')
            .long("quiet")
            .action(clap::ArgAction::SetTrue)
            .help("Disable progress output")
            .global(true))
            
        // Install command
        .subcommand(
            Command::new("install")
                .about("Install WasmEdge runtime")
                .arg(Arg::new("version")
                    .help("Version to install (default: latest)")
                    .value_name("VERSION"))
                .arg(Arg::new("path")
                    .short('p')
                    .long("path")
                    .value_name("PATH")
                    .help("Installation path")
                    .default_value("$HOME/.wasmedge"))
                .arg(Arg::new("tmpdir")
                    .short('t')
                    .long("tmpdir")
                    .value_name("DIR")
                    .help("Temporary directory")
                    .default_value("/tmp"))
                .arg(Arg::new("os")
                    .short('o')
                    .long("os")
                    .value_name("OS")
                    .help("Override OS detection")
                    .value_parser(["Linux", "Darwin", "Windows", "Ubuntu"]))
                .arg(Arg::new("arch")
                    .short('a')
                    .long("arch")
                    .value_name("ARCH")
                    .help("Override architecture detection")
                    .value_parser(["x86_64", "arm64", "aarch64"]))
        )
        
        // Remove command
        .subcommand(
            Command::new("remove")
                .about("Remove WasmEdge runtime")
                .arg(Arg::new("version")
                    .help("Version to remove (default: current)")
                    .value_name("VERSION"))
                .arg(Arg::new("all")
                    .long("all")
                    .action(clap::ArgAction::SetTrue)
                    .help("Remove all installed versions"))
        )
        
        // List command
        .subcommand(
            Command::new("list")
                .about("List WasmEdge versions")
                .arg(Arg::new("installed")
                    .long("installed")
                    .action(clap::ArgAction::SetTrue)
                    .help("List only installed versions"))
        )
        
        // Plugin commands
        .subcommand(
            Command::new("plugin")
                .about("Manage WasmEdge plugins")
                .subcommand_required(true)
                .arg_required_else_help(true)
                .subcommand(
                    Command::new("list")
                        .about("List available plugins")
                        .arg(Arg::new("installed")
                            .long("installed")
                            .action(clap::ArgAction::SetTrue)
                            .help("List only installed plugins"))
                )
                .subcommand(
                    Command::new("install")
                        .about("Install plugins")
                        .arg(Arg::new("plugins")
                            .help("Plugin names to install")
                            .value_name("PLUGIN")
                            .num_args(1..)
                            .required(true))
                        .arg(Arg::new("version")
                            .short('v')
                            .long("version")
                            .value_name("VERSION")
                            .help("Plugin version to install"))
                )
                .subcommand(
                    Command::new("remove")
                        .about("Remove plugins")
                        .arg(Arg::new("plugins")
                            .help("Plugin names to remove")
                            .value_name("PLUGIN")
                            .num_args(1..)
                            .required(true))
                )
        )
}
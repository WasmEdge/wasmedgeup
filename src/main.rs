use clap::Parser;
use wasmedgeup::cli::Cli;
use wasmedgeup::cli::CommandContext;
use wasmedgeup::cli::CommandExecutor;
use wasmedgeup::prelude::*;

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    let ctx = CommandContext::default();

    if let Some(command) = cli.commands {
        command.execute(ctx).await?;
    };

    Ok(())
}

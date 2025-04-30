use clap::Parser;
use tracing::Level;
use wasmedgeup::cli::Cli;
use wasmedgeup::cli::CommandExecutor;
use wasmedgeup::prelude::*;

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    let ctx = cli.context(Default::default());

    init_tracing(cli.verbose);

    if let Some(command) = cli.commands {
        command.execute(ctx).await?;
    };

    Ok(())
}

fn init_tracing(verbosity: u8) {
    let level = match verbosity {
        0 => Level::INFO,
        1 => Level::DEBUG,
        2.. => Level::TRACE,
    };
    tracing_subscriber::fmt().with_max_level(level).init();
}

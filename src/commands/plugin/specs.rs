use crate::cli::{CommandContext, CommandExecutor};
use crate::prelude::*;
use crate::system;
use clap::Args;

#[derive(Debug, Args)]
pub struct PluginSpecsArgs {}

impl CommandExecutor for PluginSpecsArgs {
    async fn execute(self, _ctx: CommandContext) -> Result<()> {
        let spec = system::detect();
        let json = serde_json::to_string_pretty(&spec).map_err(|_| Error::Unknown)?;
        println!("{json}");
        Ok(())
    }
}

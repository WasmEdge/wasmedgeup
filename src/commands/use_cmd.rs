use clap::Parser;
use std::path::PathBuf;

use crate::{
    cli::{CommandContext, CommandExecutor},
    commands::default_path,
    fs,
    prelude::*,
};

#[derive(Debug, Parser)]
pub struct UseArgs {
    /// WasmEdge version to use, e.g. `latest`, `0.14.1`, `0.15.0`, etc.
    pub version: String,

    /// Set the install location for the WasmEdge runtime
    ///
    /// Defaults to `$HOME/.wasmedge` on Unix-like systems and `%HOME%\.wasmedge` on Windows.
    #[arg(short, long)]
    pub path: Option<PathBuf>,
}

impl CommandExecutor for UseArgs {
    #[tracing::instrument(name = "use", skip_all, fields(version = self.version))]
    async fn execute(self, ctx: CommandContext) -> Result<()> {
        let version = ctx.client.resolve_version(&self.version).inspect_err(
            |e| tracing::error!(error = %e.to_string(), "Failed to resolve version"),
        )?;
        tracing::debug!(%version, "Resolved version for use");

        let target_dir = match self.path {
            Some(p) => p,
            None => default_path()?,
        };

        let version_dir = target_dir.join("versions").join(version.to_string());
        if !version_dir.exists() {
            return Err(Error::VersionNotFound {
                version: version.to_string(),
            });
        }

        fs::create_version_symlinks(&target_dir, &version.to_string()).await?;

        tracing::info!(%version, "Switched to WasmEdge version");
        Ok(())
    }
}

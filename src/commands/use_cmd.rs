use clap::Parser;
use snafu::ResultExt;
use std::path::PathBuf;

use crate::{
    api::latest_installed_version,
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
    async fn execute(self, _ctx: CommandContext) -> Result<()> {
        let target_dir = match self.path {
            Some(p) => p,
            None => default_path()?,
        };
        let versions_dir = target_dir.join("versions");

        // `use` switches between locally installed versions. Resolving "latest"
        // here means the highest locally installed version — hitting the network
        // (as the previous implementation did via `resolve_version`) would make
        // the command require connectivity and silently disagree with what's
        // actually on disk.
        let version = if self.version == "latest" {
            match latest_installed_version(&versions_dir)? {
                Some(v) => v,
                None => {
                    eprintln!(
                        "No WasmEdge runtime is installed. \
                        Run `wasmedgeup install latest` to install the latest version."
                    );
                    return Err(Error::VersionNotFound {
                        version: "latest".to_string(),
                    });
                }
            }
        } else {
            semver::Version::parse(&self.version).context(SemVerSnafu {})?
        };
        tracing::debug!(%version, "Resolved version for use");

        let version_dir = versions_dir.join(version.to_string());
        if !version_dir.exists() {
            eprintln!(
                "WasmEdge {version} is not installed. Run `wasmedgeup install {version}` first."
            );
            return Err(Error::VersionNotFound {
                version: version.to_string(),
            });
        }

        fs::create_version_symlinks(&target_dir, &version.to_string()).await?;

        println!("Switched to WasmEdge runtime version: {version}");
        Ok(())
    }
}

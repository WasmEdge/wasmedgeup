use clap::Parser;
use std::path::PathBuf;

use crate::{
    cli::{CommandContext, CommandExecutor},
    commands::{
        resolve_normalized_install_path,
        runtime::{open_install_root, open_versions_root, select_usable_managed_runtime},
    },
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
        let target_dir = resolve_normalized_install_path(self.path)?;
        let Some(install_root) = open_install_root(&target_dir)? else {
            return Err(Error::VersionNotFound {
                version: self.version,
            });
        };
        let Some(versions_root) = open_versions_root(&install_root, &target_dir)? else {
            return Err(Error::VersionNotFound {
                version: self.version,
            });
        };

        // `use` switches only between locally installed, usable WasmEdge
        // runtimes. Preserved foreign or marker-only directories are excluded,
        // and the selected directory handle stays pinned through link creation.
        let requested = (self.version != "latest").then_some(self.version.as_str());
        let (version, version_root) =
            select_usable_managed_runtime(&versions_root, requested, "latest")?;
        tracing::debug!(%version, "Resolved version for use");

        fs::create_version_symlinks_in(
            &install_root,
            &version_root,
            &target_dir,
            &version.to_string(),
        )?;

        println!("Switched to WasmEdge runtime version: {version}");
        Ok(())
    }
}

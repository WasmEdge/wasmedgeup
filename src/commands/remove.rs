use std::path::PathBuf;

use clap::Parser;
use tokio::fs;

use crate::{
    api::latest_installed_version,
    cli::{CommandContext, CommandExecutor},
    commands::{default_path, use_cmd::UseArgs},
    prelude::*,
    shell_utils::uninstall_path,
};

#[derive(Debug, Parser)]
pub struct RemoveArgs {
    /// WasmEdge version to remove, e.g. `0.13.0`, `0.15.0`, etc.
    #[arg(default_value = "")]
    pub version: String,

    /// Remove all installed versions
    #[arg(long)]
    pub all: bool,

    /// Set the install location for the WasmEdge runtime
    ///
    /// Defaults to `$HOME/.wasmedge` on Unix-like systems and `%HOME%\.wasmedge` on Windows.
    #[arg(short, long)]
    pub path: Option<PathBuf>,
}

impl CommandExecutor for RemoveArgs {
    async fn execute(self, ctx: CommandContext) -> Result<()> {
        let target_dir = self.path.unwrap_or_else(default_path);
        let versions_dir = target_dir.join("versions");

        if !versions_dir.exists() {
            if self.all {
                return Err(Error::InvalidPath {
                    path: versions_dir.display().to_string(),
                    reason: "no WasmEdge installation found".to_string(),
                });
            } else {
                return Err(Error::VersionNotFound {
                    version: self.version,
                });
            }
        }

        if !self.all && self.version.is_empty() {
            return Err(Error::InvalidPath {
                path: "version".to_string(),
                reason: "no version specified; provide a version or use --all".to_string(),
            });
        }

        let current_version = if target_dir.join("bin").exists() {
            let bin_link = fs::read_link(target_dir.join("bin")).await?;
            tracing::debug!(link = ?bin_link, "Raw symlink path");

            let normalized = if bin_link.is_absolute() {
                bin_link
                    .strip_prefix(&target_dir)
                    .map(|p| p.to_path_buf())
                    .unwrap_or(bin_link.clone())
            } else {
                bin_link.clone()
            };

            let mut comps = normalized.components().peekable();
            let mut found: Option<String> = None;
            while let Some(comp) = comps.next() {
                if let std::path::Component::Normal(name) = comp {
                    if name == "versions" {
                        if let Some(std::path::Component::Normal(ver)) = comps.peek().copied() {
                            let v = ver.to_string_lossy().to_string();
                            tracing::debug!(version = %v, "Extracted version from symlink");
                            found = Some(v);
                        }
                        break;
                    }
                }
            }

            if found.is_none() {
                tracing::debug!(normalized = %normalized.display(), "Could not find versions/<ver> in symlink path");
            }
            found
        } else {
            tracing::debug!("No bin symlink found");
            None
        };

        if self.all {
            tracing::debug!("Removing all installed versions");
            if let Err(e) = uninstall_path(&target_dir) {
                tracing::warn!(error = %e.to_string(), "Failed to update shell rc files during --all removal");
            }
            fs::remove_dir_all(&target_dir).await?;
            tracing::info!("All versions and configuration removed successfully");
            return Ok(());
        }

        let version = ctx.client.resolve_version(&self.version).inspect_err(
            |e| tracing::error!(error = %e.to_string(), "Failed to resolve version"),
        )?;
        tracing::debug!(%version, "Resolved version for use");

        let version_dir = versions_dir.join(version.to_string());
        if version_dir.exists() {
            fs::remove_dir_all(&version_dir).await?;
            tracing::info!(version = %version, "Version removed successfully");
        }

        let removed_current = Some(version.to_string()) == current_version;

        let mut remaining_versions = 0;
        let mut dir_stream = fs::read_dir(&versions_dir).await?;
        while let Some(entry) = dir_stream.next_entry().await? {
            if entry.file_type().await?.is_dir() {
                remaining_versions += 1;
            }
        }

        if remaining_versions == 0 {
            tracing::debug!("No versions remaining, cleaning up configuration");
            if let Err(e) = uninstall_path(&target_dir) {
                tracing::warn!(error = %e.to_string(), "Failed to update shell rc files when cleaning up last version");
            }
            fs::remove_dir_all(&target_dir).await?;
            tracing::info!("All versions and configuration removed successfully");
            return Ok(());
        }

        if removed_current && remaining_versions > 0 {
            tracing::debug!(removed_version = ?current_version, "Current version was removed");

            let latest_version = latest_installed_version(&versions_dir)?;

            if let Some(version) = latest_version {
                tracing::info!(version = %version, "Switching to latest version");
                let use_args = UseArgs {
                    version: version.to_string(),
                    path: Some(target_dir),
                };
                use_args.execute(ctx).await?;
            } else {
                tracing::warn!("No other versions found to switch to");
            }
        }

        Ok(())
    }
}

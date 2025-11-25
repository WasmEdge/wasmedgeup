use crate::{api::ReleasesFilter, cli::CommandContext, prelude::*};
use clap::Parser;
use std::path::PathBuf;
use tokio::fs;

use crate::{cli::CommandExecutor, commands::default_path};

#[derive(Debug, Parser)]
pub struct ListArgs {
    /// Show remote versions instead of installed versions
    #[arg(long, default_value_t = false)]
    remote: bool,

    /// Include pre-release versions (alpha, beta, rc) when listing remote versions
    #[arg(short, long, default_value_t = false)]
    all: bool,

    /// Set the install location for the WasmEdge runtime
    ///
    /// Defaults to `$HOME/.wasmedge` on Unix-like systems and `%HOME%\.wasmedge` on Windows.
    #[arg(short, long)]
    path: Option<PathBuf>,
}

impl CommandExecutor for ListArgs {
    async fn execute(self, ctx: CommandContext) -> Result<()> {
        if self.remote {
            let filter = if self.all {
                ReleasesFilter::All
            } else {
                ReleasesFilter::Stable
            };

            let releases = ctx.client.releases(filter, 10)?;
            let latest_release = ctx.client.latest_release()?;

            for gh_release in releases.into_iter() {
                print!("{gh_release}");
                if gh_release == latest_release {
                    println!(" <- latest");
                } else {
                    println!();
                }
            }
        } else {
            let target_dir = match self.path {
                Some(p) => p,
                None => default_path()?,
            };
            let versions_dir = target_dir.join("versions");

            let current_version =
                if let Ok(link_target) = fs::read_link(target_dir.join("bin")).await {
                    let bin_path = target_dir.join("bin");
                    let resolved = if link_target.is_absolute() {
                        link_target
                    } else {
                        bin_path.parent().unwrap_or(&target_dir).join(link_target)
                    };
                    resolved
                        .strip_prefix(&versions_dir)
                        .ok()
                        .and_then(|p| p.components().next())
                        .map(|c| c.as_os_str().to_string_lossy().to_string())
                } else {
                    None
                };

            if let Ok(mut entries) = fs::read_dir(&versions_dir).await {
                let mut versions = Vec::new();
                while let Ok(Some(entry)) = entries.next_entry().await {
                    if let Ok(file_type) = entry.file_type().await {
                        if file_type.is_dir() {
                            if let Some(version) = entry.file_name().to_str() {
                                versions.push(version.to_string());
                            }
                        }
                    }
                }

                versions.sort_by(|a, b| b.cmp(a));

                for version in versions {
                    print!("{version}");
                    if Some(version.clone()) == current_version {
                        println!(" <- current");
                    } else {
                        println!();
                    }
                }
            }
        }

        Ok(())
    }
}

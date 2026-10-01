use crate::{api::ReleasesFilter, cli::CommandContext, prelude::*};
use clap::Parser;
use semver::Version;
use std::{
    ffi::OsStr,
    path::{Component, Path, PathBuf},
};

use crate::{
    cli::CommandExecutor,
    commands::{
        resolve_normalized_install_path,
        runtime::{open_install_root, open_versions_root, usable_managed_versions},
    },
};

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

            let (releases, latest_release) = ctx.client.releases_with_latest(filter, 10).await?;

            for gh_release in releases.into_iter() {
                print!("{gh_release}");
                if gh_release == latest_release {
                    println!(" <- latest");
                } else {
                    println!();
                }
            }
        } else {
            let target_dir = resolve_normalized_install_path(self.path)?;
            let Some(install_root) = open_install_root(&target_dir)? else {
                return Ok(());
            };
            let Some(versions_root) = open_versions_root(&install_root, &target_dir)? else {
                return Ok(());
            };
            let mut versions = usable_managed_versions(&versions_root)?;
            versions.sort_by(|a, b| b.cmp(a));
            let current_version = install_root
                .read_link_contents("bin")
                .ok()
                .and_then(|target| root_link_version(&target_dir, &target))
                .filter(|current| versions.contains(current));

            for version in versions {
                print!("{version}");
                if Some(&version) == current_version.as_ref() {
                    println!(" <- current");
                } else {
                    println!();
                }
            }
        }

        Ok(())
    }
}

fn root_link_version(install_root: &Path, target: &Path) -> Option<Version> {
    let relative = if target.is_absolute() {
        target.strip_prefix(install_root).ok()?
    } else {
        target
    };
    let mut components = relative.components();
    if !matches!(components.next(), Some(Component::Normal(name)) if name == OsStr::new("versions"))
    {
        return None;
    }
    let Some(Component::Normal(version)) = components.next() else {
        return None;
    };
    if !matches!(components.next(), Some(Component::Normal(name)) if name == OsStr::new("bin"))
        || components.next().is_some()
    {
        return None;
    }
    Version::parse(version.to_str()?).ok()
}

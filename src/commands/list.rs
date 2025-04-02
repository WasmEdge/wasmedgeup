use crate::{api::ReleasesFilter, cli::CommandContext, prelude::*};
use clap::Parser;
use tokio::join;

use crate::cli::CommandExecutor;

#[derive(Debug, Parser)]
pub struct ListArgs {
    /// Include pre-release versions (alpha, beta, rc).
    #[arg(short, long, default_value_t = false)]
    all: bool,
}

impl CommandExecutor for ListArgs {
    async fn execute(self, ctx: CommandContext) -> Result<()> {
        let (gh_releases, latest_release) = join!(
            ctx.client.releases(if self.all {
                ReleasesFilter::All
            } else {
                ReleasesFilter::Stable
            }),
            ctx.client.latest_release()
        );

        let gh_releases = gh_releases?;
        let latest_release = latest_release?;

        for gh_release in gh_releases.into_iter() {
            print!("{}", gh_release.tag_name);
            if gh_release.tag_name == latest_release.tag_name {
                println!(" <- latest");
            } else {
                println!();
            }
        }

        Ok(())
    }
}

use std::sync::Arc;

use crate::prelude::*;
use futures::{future, StreamExt, TryStreamExt};
use octocrab::{models::repos::Release, Octocrab};
use snafu::ResultExt;

const WASM_EDGE_OWNER: &str = "WasmEdge";
const WASM_EDGE_REPO: &str = "WasmEdge";
const NUM_RELEASES: usize = 10;

#[derive(Debug, Clone)]
pub struct WasmEdgeApiClient {
    client: Arc<Octocrab>,
}

impl Default for WasmEdgeApiClient {
    fn default() -> Self {
        Self {
            client: octocrab::instance(),
        }
    }
}

#[derive(Debug, Clone)]
pub enum ReleasesFilter {
    All,
    Stable,
}

impl WasmEdgeApiClient {
    pub async fn releases(&self, filter: ReleasesFilter) -> Result<Vec<Release>> {
        self.client
            .repos(WASM_EDGE_OWNER, WASM_EDGE_REPO)
            .releases()
            .list()
            .send()
            .await
            .context(GitHubSnafu {
                resource: "releases",
            })?
            .into_stream(&self.client)
            .filter(|release| match (&filter, release) {
                (ReleasesFilter::Stable, Ok(r)) => future::ready(!r.prerelease),
                _ => future::ready(true),
            })
            .take(NUM_RELEASES)
            .try_collect()
            .await
            .context(GitHubSnafu {
                resource: "releases",
            })
    }

    pub async fn latest_release(&self) -> Result<Release> {
        self.client
            .repos(WASM_EDGE_OWNER, WASM_EDGE_REPO)
            .releases()
            .get_latest()
            .await
            .context(GitHubSnafu {
                resource: "releases",
            })
    }
}

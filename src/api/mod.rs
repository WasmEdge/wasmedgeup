use std::sync::Arc;

use crate::prelude::*;
use octocrab::{models::repos::Release, Octocrab, Page};
use snafu::ResultExt;

const WASM_EDGE_OWNER: &str = "WasmEdge";
const WASM_EDGE_REPO: &str = "WasmEdge";

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

impl WasmEdgeApiClient {
    pub async fn releases(&self) -> Result<Page<Release>> {
        self.client
            .repos(WASM_EDGE_OWNER, WASM_EDGE_REPO)
            .releases()
            .list()
            .send()
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

use crate::prelude::*;
use futures::{StreamExt, TryStreamExt};
use releases::Releases;
use semver::Version;

mod releases;

pub use releases::ReleasesFilter;

#[derive(Debug, Clone, Default)]
pub struct WasmEdgeApiClient {
    client: reqwest::Client,
}

impl WasmEdgeApiClient {
    pub async fn releases(
        &self,
        filter: ReleasesFilter,
        num_releases: usize,
    ) -> Result<Vec<Version>> {
        Releases::new(self.client.clone(), filter)
            .take(num_releases)
            .try_collect()
            .await
    }

    pub async fn latest_release(&self) -> Result<Version> {
        Releases::new(self.client.clone(), ReleasesFilter::Stable)
            .try_next()
            .await
            .transpose()
            .unwrap_or(Err(Error::Unknown))
    }
}

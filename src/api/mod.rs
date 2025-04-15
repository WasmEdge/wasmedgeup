use crate::prelude::*;
pub mod releases;
pub use releases::ReleasesFilter;

use semver::Version;

#[derive(Debug, Clone, Default)]
pub struct WasmEdgeApiClient {}

const WASM_EDGE_GIT_URL: &str = "https://github.com/WasmEdge/WasmEdge.git";

impl WasmEdgeApiClient {
    pub fn releases(&self, filter: ReleasesFilter, num_releases: usize) -> Result<Vec<Version>> {
        let releases = releases::get_all(WASM_EDGE_GIT_URL, filter)?;
        Ok(releases.into_iter().take(num_releases).collect())
    }

    pub fn latest_release(&self) -> Result<Version> {
        let releases = releases::get_all(WASM_EDGE_GIT_URL, ReleasesFilter::Stable)?;
        releases.into_iter().next().ok_or(Error::Unknown)
    }
}

//! Project-wide constants shared by the api, http, and command modules.
//!
//! Keeping these in one place prevents drift between parallel code paths
//! (for example, the runtime installer and the plugin subsystem both hit
//! the same GitHub release endpoints).

/// Git URL for the WasmEdge project, used by `api::releases` to enumerate
/// released versions via git refs (avoids a GitHub API rate limit hit).
pub const WASMEDGE_GIT_URL: &str = "https://github.com/WasmEdge/WasmEdge.git";

/// Base URL for downloading released WasmEdge runtime and plugin archives.
pub const WASMEDGE_RELEASE_BASE_URL: &str =
    "https://github.com/WasmEdge/WasmEdge/releases/download";

/// Base URL for the GitHub REST API endpoint that lists release metadata by tag.
pub const WASMEDGE_GH_RELEASE_TAG_API: &str =
    "https://api.github.com/repos/WasmEdge/WasmEdge/releases/tags";

/// File name of the SHA256 checksum file published alongside runtime releases.
pub const CHECKSUM_FILE_NAME: &str = "SHA256SUM";

/// Default connection timeout (seconds) for all HTTP calls.
pub const DEFAULT_CONNECT_TIMEOUT_SECS: u64 = 15;

/// Default request/read timeout (seconds) for all HTTP calls.
pub const DEFAULT_REQUEST_TIMEOUT_SECS: u64 = 90;

/// Buffer size used when streaming downloads and computing checksums.
pub const DOWNLOAD_BUFFER_SIZE: usize = 8 * 1024;

use snafu::Snafu;

#[derive(Debug, Default, Snafu)]
#[snafu(visibility(pub))]
pub enum Error {
    #[snafu(display("Version '{}' not found", version))]
    VersionNotFound { version: String },

    #[snafu(display("No versions installed"))]
    NoVersionsInstalled,

    #[snafu(display("Plugin '{}' not found", plugin))]
    PluginNotFound { plugin: String },

    #[snafu(display("Invalid version format: '{}'", version))]
    InvalidVersion { version: String },

    #[snafu(display("Download failed: {}", reason))]
    DownloadError { reason: String },

    #[snafu(display("I/O error"))]
    IO {
        source: std::io::Error,
    },

    #[snafu(display("HTTP error while requesting '{}'", resource))]
    Http {
        source: reqwest::Error,
        resource: &'static str,
    },

    #[snafu(display("JSON parsing error"))]
    Json {
        source: serde_json::Error,
    },

    #[default]
    #[snafu(display("Unknown error occurred"))]
    Unknown,
}

pub type Result<T, E = Error> = std::result::Result<T, E>;

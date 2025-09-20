use snafu::Snafu;

#[derive(Debug, Default, Snafu)]
#[snafu(visibility(pub))]
pub enum Error {
    #[snafu(display("Unable to fetch resource '{}' for git", resource))]
    Git {
        source: git2::Error,
        resource: &'static str,
    },

    #[snafu(display("Invalid semantic version specifier"))]
    SemVer { source: semver::Error },

    #[snafu(display("Error constructing release URL"))]
    Url { source: url::ParseError },

    #[snafu(display("Unable to request resource '{}'", resource))]
    Request {
        source: reqwest::Error,
        resource: &'static str,
    },

    #[snafu(display("Unable to extract archive"))]
    Extract {
        #[cfg(unix)]
        source: std::io::Error,

        #[cfg(windows)]
        source: zip::result::ZipError,
    },

    #[snafu(transparent)]
    IO { source: std::io::Error },

    #[cfg(windows)]
    #[snafu(display("Windows Registry error: {}", source))]
    WindowsRegistry { source: std::io::Error },

    #[snafu(display("Parent directory not found for rc path: {}", path))]
    RcDirNotFound { path: String },

    #[snafu(display("Checksum not found for version {} asset {}", version, asset))]
    ChecksumNotFound { version: String, asset: String },

    #[snafu(display("Checksum mismatch. Expected: {}, got: {}", expected, actual))]
    ChecksumMismatch { expected: String, actual: String },

    #[snafu(display("Invalid path {path}: {reason}"))]
    InvalidPath { path: String, reason: String },

    #[snafu(display("Failed to {action} at {path}: {source}"))]
    Io {
        action: String,
        path: String,
        source: std::io::Error,
    },

    #[default]
    #[snafu(display("Unknown error occurred"))]
    Unknown,
}

pub type Result<T, E = Error> = std::result::Result<T, E>;

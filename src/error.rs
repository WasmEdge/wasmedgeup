use snafu::Snafu;

#[derive(Debug, Default, Snafu)]
#[snafu(visibility(pub))]
pub enum Error {
    #[snafu(display("Version {version} not found in wasmedge installation"))]
    VersionNotFound { version: String },

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

    #[snafu(display("Error: Cannot {action} at {path}\n\nTo install WasmEdge system-wide:\n  1. Install to {system_dir} (recommended):\n     {sudo}wasmedgeup install {version} -p {system_dir}\n\n  2. Install to user directory (default: $HOME/.wasmedge):\n     wasmedgeup install {version}\n\nInstalling to system directories requires administrator privileges."))]
    InsufficientPermissions {
        path: String,
        action: String,
        version: String,
        system_dir: String,
        sudo: String,
    },

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

    #[snafu(display("Unsupported platform: os={os} arch={arch}"))]
    UnsupportedPlatform { os: String, arch: String },

    #[snafu(display(
        "WasmEdge runtime not found on PATH; please install WasmEdge or ensure PATH is set"
    ))]
    RuntimeNotFound,

    #[default]
    #[snafu(display("Unknown error occurred"))]
    Unknown,

    #[snafu(display("No plugins specified for installation"))]
    NoPluginsSpecified,

    #[cfg(windows)]
    #[snafu(display("Error: Cannot create symbolic links.\n\nTo enable symlink creation on Windows:\n  1. Run as Administrator, or\n  2. Enable Developer Mode:\n     - Open Windows Settings\n     - Update & Security > For developers\n     - Enable 'Developer Mode'\n"))]
    WindowsSymlinkError { version: String },

    #[snafu(display("Invalid archive structure: found '{found_file}' but expected either a WasmEdge directory or standard directories (bin, lib64, include, lib).\n\nThis might indicate:\n  1. A corrupted download\n  2. An unsupported archive format\n  3. A change in the WasmEdge release structure"))]
    InvalidArchiveStructure { found_file: String },
}

pub type Result<T, E = Error> = std::result::Result<T, E>;

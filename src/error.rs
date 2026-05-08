use snafu::Snafu;

#[derive(Debug, Snafu)]
#[snafu(visibility(pub))]
pub enum Error {
    #[snafu(display("Version {version} not found in wasmedge installation"))]
    VersionNotFound { version: String },

    #[snafu(display("Unable to fetch resource '{}' for git: {}", resource, source))]
    Git {
        source: Box<dyn std::error::Error + Send + Sync + 'static>,
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

    #[snafu(display("Unable to parse JSON from '{}'", resource))]
    Json {
        source: serde_json::Error,
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

    #[snafu(display("No WasmEdge releases were found"))]
    NoReleasesFound,

    #[snafu(display("No plugins specified for installation"))]
    NoPluginsSpecified,

    #[cfg(windows)]
    #[snafu(display("Error: Cannot create symbolic links.\n\nTo enable symlink creation on Windows:\n  1. Run as Administrator, or\n  2. Enable Developer Mode:\n     - Open Windows Settings\n     - Update & Security > For developers\n     - Enable 'Developer Mode'\n"))]
    WindowsSymlinkError { version: String },

    #[snafu(display("Invalid archive structure: found '{found_file}' but expected either a WasmEdge directory or standard directories (bin, lib64, include, lib).\n\nThis might indicate:\n  1. A corrupted download\n  2. An unsupported archive format\n  3. A change in the WasmEdge release structure"))]
    InvalidArchiveStructure { found_file: String },

    #[snafu(display(
        "Home directory could not be determined. Please specify an installation path using --path"
    ))]
    HomeDirNotFound,

    #[snafu(display("Failed to build HTTP client: {reason}"))]
    HttpClientBuild { reason: String },
}

pub type Result<T, E = Error> = std::result::Result<T, E>;

/// Convert a [`tokio::task::JoinError`] into a [`std::io::Error`] without
/// losing diagnostic context.
///
/// `JoinError`'s `Display` impl renders only `"task was cancelled"` or
/// `"task panicked"` — the panic payload is stored separately in the
/// internal `Box<dyn Any + Send>` and is unreachable through `to_string()`.
/// The previous call sites used `std::io::Error::other(join_err.to_string())`,
/// which both stringified prematurely *and* discarded the panic payload.
///
/// This helper:
///
/// - For panics: extracts the payload via [`JoinError::try_into_panic`] and
///   includes the message in the resulting `io::Error` (handling both the
///   `&'static str` and `String` cases that `panic!` produces in practice).
/// - For cancellations: preserves the original `JoinError` as the `io::Error`
///   source, so downstream callers can still downcast and inspect via
///   `io_err.get_ref()`.
pub(crate) fn join_err_to_io_error(join_err: tokio::task::JoinError) -> std::io::Error {
    if join_err.is_panic() {
        // Documented contract: when is_panic() is true, try_into_panic returns
        // Ok. The Err arm here is defense-in-depth for future tokio changes.
        let payload = match join_err.try_into_panic() {
            Ok(p) => p,
            Err(_) => return std::io::Error::other("blocking task panicked (payload unavailable)"),
        };
        let msg = if let Some(s) = payload.downcast_ref::<&'static str>() {
            (*s).to_string()
        } else if let Some(s) = payload.downcast_ref::<String>() {
            s.clone()
        } else {
            "<non-string panic payload>".to_string()
        };
        std::io::Error::other(format!("blocking task panicked: {msg}"))
    } else {
        // Cancellation: preserve the JoinError as source so callers can
        // still downcast it (io_err.get_ref().and_then(|e| e.downcast_ref())).
        std::io::Error::other(join_err)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn join_err_to_io_extracts_str_panic_payload() {
        let join_err = tokio::task::spawn_blocking(|| panic!("custom panic literal"))
            .await
            .expect_err("the task panicked, so awaiting must yield Err");
        let io_err = join_err_to_io_error(join_err);
        assert!(
            io_err.to_string().contains("custom panic literal"),
            "io_err display `{io_err}` should include the &'static str payload"
        );
    }

    #[tokio::test]
    async fn join_err_to_io_extracts_string_panic_payload() {
        let join_err = tokio::task::spawn_blocking(|| panic!("formatted panic: {}", 42))
            .await
            .expect_err("the task panicked, so awaiting must yield Err");
        let io_err = join_err_to_io_error(join_err);
        assert!(
            io_err.to_string().contains("42"),
            "io_err display `{io_err}` should include the formatted String payload"
        );
    }

    #[tokio::test]
    async fn join_err_to_io_preserves_cancellation_as_source() {
        let handle = tokio::task::spawn(async {
            tokio::time::sleep(std::time::Duration::from_secs(60)).await;
        });
        handle.abort();
        let join_err = handle.await.expect_err("aborted task must yield Err");
        assert!(
            join_err.is_cancelled(),
            "test setup: must be a cancellation"
        );
        let io_err = join_err_to_io_error(join_err);
        let source = io_err
            .get_ref()
            .expect("cancellation path should preserve the JoinError as source");
        assert!(
            source.to_string().contains("cancel"),
            "source display `{source}` should mention cancellation"
        );
    }
}

use crate::system::spec::{LibcKind, ToolchainSpec};
use std::path::PathBuf;
use std::process::Command;

pub fn detect_toolchain(
    libc_kind: LibcKind,
    libc_version: Option<String>,
) -> (ToolchainSpec, Vec<String>, Vec<String>) {
    let notes = Vec::new();
    let errors = Vec::new();

    let nvidia_smi_path = which("nvidia-smi");
    let nvcc_path = which("nvcc");
    let rocminfo_path = which("rocminfo");
    let clinfo_path = which("clinfo");
    let vulkaninfo_path = which("vulkaninfo");

    let toolchain = ToolchainSpec {
        nvidia_smi_path,
        nvcc_path,
        rocminfo_path,
        clinfo_path,
        vulkaninfo_path,
        libc_kind,
        libc_version,
    };

    (toolchain, notes, errors)
}

fn which(bin: &str) -> Option<PathBuf> {
    std::env::var_os("PATH").and_then(|paths| {
        std::env::split_paths(&paths).find_map(|p| {
            let candidate = p.join(bin);
            if candidate.exists() {
                Some(candidate)
            } else {
                None
            }
        })
    })
}

pub fn get_installed_wasmedge_version() -> Result<String, String> {
    let out = Command::new("wasmedge")
        .arg("--version")
        .output()
        .map_err(|e| format!("failed to exec wasmedge: {e}"))?;
    if !out.status.success() {
        return Err("wasmedge --version exited with non-zero status".to_string());
    }
    let stdout = String::from_utf8_lossy(&out.stdout);
    parse_wasmedge_version_output(&stdout)
        .ok_or_else(|| format!("unable to parse version from: {}", stdout.trim()))
}

/// Extract a semver-like version token from `wasmedge --version` output.
///
/// Picks the first whitespace-separated token that starts with an ASCII
/// digit and contains at least one `.`, then trims trailing characters
/// that are not alphanumerics, `.`, or `-`. Returns `None` if no token
/// matches.
pub(crate) fn parse_wasmedge_version_output(stdout: &str) -> Option<String> {
    for token in stdout.split_whitespace() {
        let starts_with_digit = token
            .chars()
            .next()
            .map(|c| c.is_ascii_digit())
            .unwrap_or(false);
        if starts_with_digit && token.contains('.') {
            let ver = token
                .trim_end_matches(|c: char| !c.is_ascii_alphanumeric() && c != '.' && c != '-');
            return Some(ver.to_string());
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::parse_wasmedge_version_output;

    #[test]
    fn parses_simple_version() {
        assert_eq!(
            parse_wasmedge_version_output("wasmedge version 0.15.0"),
            Some("0.15.0".to_string())
        );
    }

    #[test]
    fn parses_prerelease_version() {
        assert_eq!(
            parse_wasmedge_version_output("wasmedge version 0.15.0-rc.1"),
            Some("0.15.0-rc.1".to_string())
        );
    }

    #[test]
    fn trims_trailing_non_version_chars() {
        // Characters other than alphanumerics, '.', and '-' should be trimmed
        // from the tail; a trailing comma/paren is stripped while a legitimate
        // trailing '.' or '-' would be preserved.
        assert_eq!(
            parse_wasmedge_version_output("wasmedge version 0.15.0, build info"),
            Some("0.15.0".to_string())
        );
        assert_eq!(
            parse_wasmedge_version_output("wasmedge (version 0.15.0)"),
            Some("0.15.0".to_string())
        );
    }

    #[test]
    fn returns_none_for_no_version_token() {
        assert_eq!(parse_wasmedge_version_output("wasmedge (no version)"), None);
    }

    #[test]
    fn returns_none_for_empty_output() {
        assert_eq!(parse_wasmedge_version_output(""), None);
    }

    #[test]
    fn ignores_leading_non_digit_tokens() {
        assert_eq!(
            parse_wasmedge_version_output("version: 0.14.1"),
            Some("0.14.1".to_string())
        );
    }
}

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
    // Heuristic parse: pick the first token that starts with a digit and contains at least one '.'
    // e.g., "wasmedge version 0.15.0" -> 0.15.0
    // also accept prerelease like 0.15.0-rc.1
    for token in stdout.split_whitespace() {
        if token
            .chars()
            .next()
            .map(|c| c.is_ascii_digit())
            .unwrap_or(false)
            && token.contains('.')
        {
            let ver = token
                .trim_end_matches(|c: char| !c.is_ascii_alphanumeric() && c != '.' && c != '-');
            return Ok(ver.to_string());
        }
    }
    Err(format!("unable to parse version from: {}", stdout.trim()))
}

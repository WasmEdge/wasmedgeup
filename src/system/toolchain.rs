use crate::system::spec::{LibcKind, ToolchainSpec};
use std::path::PathBuf;

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

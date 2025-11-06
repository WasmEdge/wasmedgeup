use serde::Serialize;
use std::collections::HashSet;
use std::path::PathBuf;

use crate::target::{TargetArch, TargetOS};

#[derive(Debug, Clone, Serialize)]
pub struct SystemSpec {
    pub os: OsSpec,
    pub cpu: CpuSpec,
    pub gpus: Vec<GpuSpec>,
    pub accelerators: AcceleratorSupport,
    pub toolchain: ToolchainSpec,
    pub target_triple: String,
    pub notes: Vec<String>,
    pub detection_errors: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct OsSpec {
    pub os_type: TargetOS,
    pub arch: TargetArch,
    pub distro: Option<String>,
    pub version: Option<String>,
    pub kernel: Option<String>,
    pub libc: LibcSpec,
}

#[derive(Debug, Clone, Serialize)]
pub struct LibcSpec {
    pub kind: LibcKind,
    pub version: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize)]
pub enum LibcKind {
    Glibc,
    Musl,
    Unknown,
}

#[derive(Debug, Clone, Serialize)]
pub struct CpuSpec {
    pub arch: TargetArch,
    pub vendor: Option<String>,
    pub model: Option<String>,
    pub cores_physical: Option<u32>,
    pub cores_logical: Option<u32>,
    pub features: HashSet<CpuFeature>,
    pub class: CpuClass,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
pub enum CpuFeature {
    // x86
    SSE2,
    SSE4_1,
    SSE4_2,
    AVX,
    AVX2,
    AVX512,
    FMA,
    BMI1,
    BMI2,
    AESNI,
    POPCNT,
    // ARM
    NEON,
    SVE,
    SVE2,
}

#[derive(Debug, Clone, Copy, Serialize)]
pub enum CpuClass {
    Avx512,
    Avx2,
    Avx,
    NoAvx,
    Neon,
    NeonOnly,
    Sve,
    Sve2,
    Generic,
}

#[derive(Debug, Clone, Serialize)]
pub struct GpuSpec {
    pub vendor: GpuVendor,
    pub model: Option<String>,
    pub vram_mb: Option<u32>,
    pub bus: Option<String>,
    pub cuda: Option<CudaSpec>,
    pub rocm: Option<RocmSpec>,
    pub opencl: Option<OpenClDeviceSpec>,
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
pub enum GpuVendor {
    Nvidia,
    AMD,
    Intel,
    Other,
}

#[derive(Debug, Clone, Serialize)]
pub struct CudaSpec {
    pub driver_version: Option<String>,
    pub runtime_version: Option<String>,
    pub compute_capability: Option<String>,
    pub device_uuid: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct RocmSpec {
    pub rocm_version: Option<String>,
    pub gfx_arch: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct OpenClDeviceSpec {
    pub platform: String,
    pub vendor: String,
    pub version: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct AcceleratorSupport {
    pub cuda_available: bool,
    pub rocm_available: bool,
    pub opencl_available: bool,
    pub vulkan_available: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct ToolchainSpec {
    pub nvidia_smi_path: Option<PathBuf>,
    pub nvcc_path: Option<PathBuf>,
    pub rocminfo_path: Option<PathBuf>,
    pub clinfo_path: Option<PathBuf>,
    pub vulkaninfo_path: Option<PathBuf>,
    pub libc_kind: LibcKind,
    pub libc_version: Option<String>,
}

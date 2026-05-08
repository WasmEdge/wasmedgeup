pub mod cpu;
pub mod detector;
pub mod gpu;
pub mod os;
pub mod plugins;
pub mod spec;
pub mod toolchain;

pub use detector::detect;
pub use spec::{
    AcceleratorSupport, CpuClass, CpuFeature, CpuSpec, CudaSpec, GpuSpec, GpuVendor, LibcKind,
    LibcSpec, OpenClDeviceSpec, OsSpec, RocmSpec, SystemSpec, ToolchainSpec,
};

/// Resolve `bin` via the OS `PATH` and return the path to the executable
/// as reported by the [`which`] crate, or `None` if no match is found.
///
/// On Windows the resolution honours `PATHEXT`, so passing a bare name
/// like `"wasmedge"` will locate `wasmedge.exe` (or any other configured
/// executable suffix). The returned path is whatever `which::which`
/// produces — usually absolute, but in unusual setups it may be relative
/// (e.g. when `PATH` contains relative entries, or when `bin` already
/// contains path separators and is used as-is). Callers that need a
/// canonical absolute form should layer their own `canonicalize()` on top.
///
/// Thin wrapper over the `which` crate so detection code doesn't have to
/// import it directly, and so gpu/toolchain modules no longer maintain
/// parallel copies of the same helper.
pub(crate) fn which_bin(bin: &str) -> Option<std::path::PathBuf> {
    which::which(bin).ok()
}

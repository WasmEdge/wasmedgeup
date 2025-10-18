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

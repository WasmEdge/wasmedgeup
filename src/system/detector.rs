use crate::system::cpu::detect_cpu;
use crate::system::gpu::detect_gpu;
use crate::system::os::detect_os;
use crate::system::spec::{LibcKind, SystemSpec};
use crate::system::toolchain::detect_toolchain;
use crate::target::TargetArch;

pub fn detect() -> SystemSpec {
    let (os, mut notes, mut errors) = detect_os();
    let (cpu, n2, e2) = detect_cpu();
    let (gpus, accelerators, n3, e3) = detect_gpu();
    let (toolchain, n4, e4) = detect_toolchain(os.libc.kind, os.libc.version.clone());

    notes.extend(n2);
    notes.extend(n3);
    notes.extend(n4);

    errors.extend(e2);
    errors.extend(e3);
    errors.extend(e4);

    let target_triple = compute_target_triple(os.os_type, os.arch, os.libc.kind);

    SystemSpec {
        os,
        cpu,
        gpus,
        accelerators,
        toolchain,
        target_triple,
        notes,
        detection_errors: errors,
    }
}

fn compute_target_triple(os: crate::target::TargetOS, arch: TargetArch, libc: LibcKind) -> String {
    let arch_str = match arch {
        TargetArch::X86_64 => "x86_64",
        TargetArch::Aarch64 => "aarch64",
    };

    match os {
        crate::target::TargetOS::Linux | crate::target::TargetOS::Ubuntu => {
            let abi = match libc {
                LibcKind::Musl => "musl",
                _ => "gnu",
            };
            format!("{arch_str}-unknown-linux-{abi}")
        }
        crate::target::TargetOS::Darwin => {
            format!("{arch_str}-apple-darwin")
        }
        crate::target::TargetOS::Windows => {
            // MSVC by default
            format!("{arch_str}-pc-windows-msvc")
        }
    }
}

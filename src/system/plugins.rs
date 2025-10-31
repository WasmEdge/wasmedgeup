use crate::error::{Error, Result};
use crate::system::spec::{LibcKind, OsSpec};
use crate::target::{TargetArch, TargetOS};

pub fn platform_key_from_specs(os: &OsSpec) -> Result<String> {
    let arch_str = match os.arch {
        TargetArch::X86_64 => "x86_64",
        TargetArch::Aarch64 => "aarch64",
    };
    match os.os_type {
        TargetOS::Darwin => {
            let a = if matches!(os.arch, TargetArch::Aarch64) {
                "arm64"
            } else {
                arch_str
            };
            Ok(format!("darwin_{a}"))
        }
        TargetOS::Windows => Ok("windows_x86_64".to_string()),
        TargetOS::Linux | TargetOS::Ubuntu => {
            let distro = os.distro.as_deref().unwrap_or("").to_lowercase();
            let version = os.version.as_deref().unwrap_or("");
            if distro.contains("ubuntu") {
                if version.starts_with("20.04") || version.starts_with("20") {
                    return Ok(format!("ubuntu20_04_{arch_str}"));
                }
                if version.starts_with("22.04") || version.starts_with("22") {
                    return Ok(format!("ubuntu22_04_{arch_str}"));
                }
            }
            if matches!(os.libc.kind, LibcKind::Glibc) {
                return Ok(format!("manylinux_2_28_{arch_str}"));
            }
            Err(Error::UnsupportedPlatform {
                os: format!("{:?}", os.os_type),
                arch: format!("{:?}", os.arch),
            })
        }
    }
}

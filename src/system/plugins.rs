use crate::error::{Error, Result};
use crate::system::spec::{LibcKind, OsSpec};
use crate::target::{TargetArch, TargetOS};
use semver::Version;

/// Compute the plugin platform key for a given OS spec and target WasmEdge runtime version.
///
/// Rules:
/// - macOS: darwin_<darwin-major>-<arch> when available (fallback: darwin_<arch>)
/// - Windows: windows_x86_64 on x86_64
/// - Linux (glibc):
///   - <= 0.14.x: manylinux2014_<arch>
///   - >= 0.15.x: manylinux_2_28_<arch>
pub fn plugin_platform_key(os: &OsSpec, runtime_version: &Version) -> Result<String> {
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
            if let Some(ver) = &os.version {
                if let Some((major, _rest)) = ver.split_once('.') {
                    if !major.is_empty() && major.chars().all(|c| c.is_ascii_digit()) {
                        return Ok(format!("darwin_{}-{}", major, a));
                    }
                }
            }
            // Fallback to generic darwin_<arch>
            Ok(format!("darwin_{a}"))
        }
        TargetOS::Windows => match os.arch {
            TargetArch::X86_64 => Ok("windows_x86_64".to_string()),
            _ => Err(Error::UnsupportedPlatform {
                os: "Windows".to_string(),
                arch: format!("{:?}", os.arch),
            }),
        },
        TargetOS::Linux | TargetOS::Ubuntu => {
            if matches!(os.libc.kind, LibcKind::Glibc) {
                let rc_boundary =
                    Version::parse("0.15.0-rc.0").map_err(|source| Error::SemVer { source })?;
                let use_ml2014 = runtime_version < &rc_boundary;
                let key = if use_ml2014 {
                    format!("manylinux2014_{arch_str}")
                } else {
                    format!("manylinux_2_28_{arch_str}")
                };
                Ok(key)
            } else {
                Err(Error::UnsupportedPlatform {
                    os: format!("{:?}", os.os_type),
                    arch: format!("{:?}", os.arch),
                })
            }
        }
    }
}

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

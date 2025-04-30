use clap::ValueEnum;

#[derive(Debug, Clone, ValueEnum)]
pub enum TargetOS {
    Linux,
    Ubuntu,
    /// aliases: [darwin, macos]
    #[value(alias("macos"))]
    Darwin,
    Windows,
}

impl Default for TargetOS {
    fn default() -> Self {
        cfg_if::cfg_if! {
            if #[cfg(target_os = "linux")] {
                match get_ubuntu_version() {
                    Some((20, minor)) if minor >= 4 => Self::Ubuntu,
                    Some((major, _)) if major > 20 => Self::Ubuntu,
                    _ => Self::Linux
                }
            } else if #[cfg(target_os = "macos")] {
                Self::Darwin
            } else if #[cfg(target_os = "windows")] {
                Self::Windows
            } else {
                compile_error!("Unsupported target OS: '{}'", std::env::consts::OS);
            }
        }
    }
}

macro_rules! unwrap_or_continue {
    ($expr:expr, $variant:ident) => {
        match $expr {
            $variant(v) => v,
            _ => continue,
        }
    };
}

#[cfg(target_os = "linux")]
fn get_ubuntu_version() -> Option<(u32, u32)> {
    use std::fs;

    let Ok(lsb_release) = fs::read_to_string("/etc/lsb-release") else {
        return None;
    };

    for line in lsb_release.lines() {
        let (key, value) = unwrap_or_continue!(line.split_once('='), Some);

        if key.eq_ignore_ascii_case("RELEASE") {
            let (major, minor) = unwrap_or_continue!(value.split_once('.'), Some);
            let major = unwrap_or_continue!(major.parse(), Ok);
            let minor = unwrap_or_continue!(minor.parse(), Ok);
            return Some((major, minor));
        }

        if key.eq_ignore_ascii_case("DESCRIPTION") && value.contains("Ubuntu 20.04") {
            return Some((20, 4));
        }
    }

    None
}

#[derive(Debug, Clone, ValueEnum)]
pub enum TargetArch {
    /// aliases: [x86_64, amd64]
    #[value(name = "x86_64", alias("amd64"))]
    X86_64,

    /// aliases: [aarch64, arm64]
    #[value(alias("arm64"))]
    Aarch64,
}

impl Default for TargetArch {
    fn default() -> Self {
        cfg_if::cfg_if! {
            if #[cfg(target_arch = "x86_64")] {
                Self::X86_64
            } else if #[cfg(target_arch = "aarch64")] {
                Self::Aarch64
            } else {
                compile_error!("Unsupported target architecture: {}", std::env::consts::ARCH);
            }
        }
    }
}

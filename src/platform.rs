use std::env;

#[derive(Debug, Clone)]
pub struct PlatformInfo {
    pub os: String,
    pub arch: String,
    pub distro: Option<String>,
}

impl PlatformInfo {
    pub fn detect() -> Self {
        let os = Self::detect_os();
        let arch = Self::detect_arch();
        let distro = Self::detect_distro();
        
        Self { os, arch, distro }
    }
    
    fn detect_os() -> String {
        match env::consts::OS {
            "linux" => {
                if Self::is_ubuntu() {
                    "Ubuntu".to_string()
                } else {
                    "Linux".to_string()
                }
            },
            "macos" => "Darwin".to_string(),
            "windows" => "Windows".to_string(),
            other => other.to_string(),
        }
    }
    
    fn detect_arch() -> String {
        match env::consts::ARCH {
            "x86_64" => "x86_64".to_string(),
            "aarch64" => "aarch64".to_string(),
            "arm64" => "aarch64".to_string(),
            other => other.to_string(),
        }
    }
    
    fn detect_distro() -> Option<String> {
        #[cfg(target_os = "linux")]
        {
            if let Ok(content) = std::fs::read_to_string("/etc/os-release") {
                if content.contains("Ubuntu") {
                    return Some("Ubuntu".to_string());
                }
            }
        }
        None
    }
    
    fn is_ubuntu() -> bool {
        #[cfg(target_os = "linux")]
        {
            std::fs::read_to_string("/etc/os-release")
                .map(|s| s.contains("Ubuntu"))
                .unwrap_or(false)
        }
        #[cfg(not(target_os = "linux"))]
        false
    }
    
    pub fn to_platform_string(&self) -> String {
        match (self.os.as_str(), self.arch.as_str()) {
            ("Ubuntu", "x86_64") => "ubuntu20_04_x86_64".to_string(),
            ("Ubuntu", "aarch64") => "ubuntu20_04_aarch64".to_string(),
            ("Linux", "x86_64") => "manylinux_2_28_x86_64".to_string(),
            ("Linux", "aarch64") => "manylinux_2_28_aarch64".to_string(),
            ("Darwin", "x86_64") => "darwin_x86_64".to_string(),
            ("Darwin", "aarch64") => "darwin_arm64".to_string(),
            ("Windows", "x86_64") => "windows_x86_64".to_string(),
            _ => format!("{}_{}", self.os.to_lowercase(), self.arch),
        }
    }

    pub fn to_plugin_platform_string(&self) -> String {
        match (self.os.as_str(), self.arch.as_str()) {
            ("Linux", "x86_64") => "linux-x86_64".to_string(),
            ("Linux", "aarch64") => "linux-aarch64".to_string(),
            ("Darwin", "x86_64") => "darwin-x86_64".to_string(),
            ("Darwin", "aarch64") => "darwin-arm64".to_string(),
            ("Windows", "x86_64") => "windows-x86_64".to_string(),
            _ => format!("{}-{}", self.os.to_lowercase(), self.arch),
        }
    }
}
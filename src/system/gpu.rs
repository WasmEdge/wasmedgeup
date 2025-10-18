use crate::system::spec::{
    AcceleratorSupport, CudaSpec, GpuSpec, GpuVendor, OpenClDeviceSpec, RocmSpec,
};
use std::path::PathBuf;

#[cfg(unix)]
use std::process::Command;

#[cfg(windows)]
use nvml_wrapper::Nvml;
#[cfg(all(windows, feature = "opencl"))]
use opencl3::platform::{get_platforms, Platform};
#[cfg(windows)]
use serde::Deserialize;
#[cfg(windows)]
use wmi::{COMLibrary, WMIConnection};

#[cfg(windows)]
#[derive(Deserialize)]
#[serde(rename = "Win32_VideoController")]
struct VideoController {
    #[serde(rename = "Name")]
    name: Option<String>,
    #[serde(rename = "AdapterRAM")]
    adapter_ram: Option<i64>,
}

pub fn detect_gpu() -> (Vec<GpuSpec>, AcceleratorSupport, Vec<String>, Vec<String>) {
    #[cfg(unix)]
    let notes = Vec::new();
    #[cfg(unix)]
    let mut errors = Vec::new();
    #[cfg(windows)]
    let notes = Vec::new();
    #[cfg(windows)]
    let mut errors = Vec::new();

    let nvidia_smi = which("nvidia-smi");
    #[cfg(unix)]
    let rocminfo = which("rocminfo");
    let clinfo = which("clinfo");
    let vulkaninfo = which("vulkaninfo");

    let mut gpus: Vec<GpuSpec> = Vec::new();

    #[cfg(unix)]
    {
        // NVIDIA via nvidia-smi
        if let Some(path) = nvidia_smi.clone() {
            match query_nvidia_smi(&path) {
                Ok(mut list) => gpus.append(&mut list),
                Err(e) => errors.push(format!("nvidia-smi: {e}")),
            }
        }

        // ROCm via rocminfo
        if let Some(path) = rocminfo.clone() {
            match query_rocminfo(&path) {
                Ok(mut list) => gpus.append(&mut list),
                Err(e) => errors.push(format!("rocminfo: {e}")),
            }
        }
    }

    #[cfg(unix)]
    {
        // OpenCL summary via clinfo
        let opencl_available = clinfo.is_some();
        if let Some(path) = clinfo.clone() {
            if gpus.is_empty() {
                // add minimal OpenCL device entry if nothing else detected
                if let Ok(mut list) = query_clinfo_minimal(&path) {
                    gpus.append(&mut list);
                }
            }
        }

        let accelerators = AcceleratorSupport {
            cuda_available: nvidia_smi.is_some(),
            rocm_available: rocminfo.is_some(),
            opencl_available,
            vulkan_available: vulkaninfo.is_some(),
        };
        return (gpus, accelerators, notes, errors);
    }

    #[cfg(windows)]
    {
        // Windows path: prefer NVML, then OpenCL, then WMI fallback
        // NVML
        match Nvml::init() {
            Ok(nvml) => {
                if let Ok(count) = nvml.device_count() {
                    for i in 0..count {
                        if let Ok(dev) = nvml.device_by_index(i) {
                            let name = dev.name().ok().map(|s| s.to_string());
                            let mem = dev
                                .memory_info()
                                .ok()
                                .map(|m| (m.total / (1024 * 1024)) as u32);
                            let driver = nvml.sys_driver_version().ok().map(|s| s.to_string());
                            let uuid = dev.uuid().ok().map(|s| s.to_string());
                            let cuda = Some(CudaSpec {
                                driver_version: driver,
                                runtime_version: None,
                                compute_capability: None,
                                device_uuid: uuid,
                            });
                            gpus.push(GpuSpec {
                                vendor: GpuVendor::Nvidia,
                                model: name,
                                vram_mb: mem,
                                bus: None,
                                cuda,
                                rocm: None,
                                opencl: None,
                            });
                        }
                    }
                }
            }
            Err(e) => {
                errors.push(format!("nvml init failed: {e}"));
            }
        }

        // OpenCL via opencl3 (optional feature)
        #[cfg(feature = "opencl")]
        let mut opencl_available = false;
        #[cfg(not(feature = "opencl"))]
        let mut opencl_available = false;
        #[cfg(feature = "opencl")]
        {
            if let Ok(platforms) = get_platforms() {
                if !platforms.is_empty() {
                    opencl_available = true;
                    if gpus.is_empty() {
                        // Add minimal OpenCL entry for first platform
                        let p = platforms[0];
                        let plat = Platform::new(p.into());
                        let pname = plat.name().ok().unwrap_or_default();
                        let pvend = plat.vendor().ok().unwrap_or_default();
                        let pver = plat.version().ok().unwrap_or_default();
                        gpus.push(GpuSpec {
                            vendor: vendor_from_str(&pvend),
                            model: None,
                            vram_mb: None,
                            bus: None,
                            cuda: None,
                            rocm: None,
                            opencl: Some(OpenClDeviceSpec {
                                platform: pname,
                                vendor: pvend,
                                version: pver,
                            }),
                        });
                    }
                }
            }
        }
        // WMI fallback if no GPUs found
        if gpus.is_empty() {
            if let Ok(com) = COMLibrary::new() {
                if let Ok(wmi_con) = WMIConnection::new(com.into()) {
                    if let Ok(results) = wmi_con.query::<VideoController>() {
                        for v in results {
                            let name: Option<String> = v.name;
                            let ram_mb: Option<u32> =
                                v.adapter_ram.map(|n| (n as u64 / (1024 * 1024)) as u32);
                            gpus.push(GpuSpec {
                                vendor: name
                                    .as_ref()
                                    .map(|s| vendor_from_str(s))
                                    .unwrap_or(GpuVendor::Other),
                                model: name,
                                vram_mb: ram_mb,
                                bus: None,
                                cuda: None,
                                rocm: None,
                                opencl: None,
                            });
                        }
                    }
                }
            }
        }

        let accelerators = AcceleratorSupport {
            cuda_available: !gpus.is_empty()
                && gpus
                    .iter()
                    .any(|g| matches!(g.vendor, GpuVendor::Nvidia) && g.cuda.is_some()),
            rocm_available: false,
            opencl_available,
            vulkan_available: vulkaninfo.is_some(),
        };
        return (gpus, accelerators, notes, errors);
    }

    #[allow(unreachable_code)]
    (
        gpus,
        AcceleratorSupport {
            cuda_available: false,
            rocm_available: false,
            opencl_available: false,
            vulkan_available: false,
        },
        Vec::new(),
        Vec::new(),
    )
}

#[cfg(windows)]
fn vendor_from_str(s: &str) -> GpuVendor {
    let l = s.to_lowercase();
    if l.contains("nvidia") {
        GpuVendor::Nvidia
    } else if l.contains("advanced micro devices") || l.contains("amd") {
        GpuVendor::AMD
    } else if l.contains("intel") {
        GpuVendor::Intel
    } else {
        GpuVendor::Other
    }
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

#[cfg(unix)]
fn query_nvidia_smi(path: &PathBuf) -> Result<Vec<GpuSpec>, String> {
    let out = Command::new(path)
        .args([
            "--query-gpu=name,uuid,memory.total,driver_version,compute_cap",
            "--format=csv,noheader,nounits",
        ])
        .output()
        .map_err(|e| e.to_string())?;
    if !out.status.success() {
        return Err("nvidia-smi failed".into());
    }
    let s = String::from_utf8_lossy(&out.stdout);
    let mut list = Vec::new();
    for line in s.lines() {
        let cols: Vec<_> = line.split(',').map(|c| c.trim()).collect();
        if cols.len() < 5 {
            continue;
        }
        let model = Some(cols[0].to_string());
        let device_uuid = Some(cols[1].to_string());
        let vram_mb = cols[2].parse::<u32>().ok();
        let driver_version = Some(cols[3].to_string());
        let compute_capability = Some(cols[4].to_string());
        let cuda = Some(CudaSpec {
            driver_version,
            runtime_version: None,
            compute_capability,
            device_uuid,
        });
        list.push(GpuSpec {
            vendor: GpuVendor::Nvidia,
            model,
            vram_mb,
            bus: None,
            cuda,
            rocm: None,
            opencl: None,
        });
    }
    Ok(list)
}

#[cfg(unix)]
fn query_rocminfo(path: &PathBuf) -> Result<Vec<GpuSpec>, String> {
    let out = Command::new(path).output().map_err(|e| e.to_string())?;
    if !out.status.success() {
        return Err("rocminfo failed".into());
    }
    let s = String::from_utf8_lossy(&out.stdout);
    let mut list = Vec::new();
    for line in s.lines() {
        if let Some(idx) = line.find("gfx") {
            let token = &line[idx..];
            let gfx = token.split_whitespace().next().unwrap_or("").to_string();
            list.push(GpuSpec {
                vendor: GpuVendor::AMD,
                model: None,
                vram_mb: None,
                bus: None,
                cuda: None,
                rocm: Some(RocmSpec {
                    rocm_version: None,
                    gfx_arch: Some(gfx),
                }),
                opencl: None,
            });
            break;
        }
    }
    Ok(list)
}

#[cfg(unix)]
fn query_clinfo_minimal(path: &PathBuf) -> Result<Vec<GpuSpec>, String> {
    let out = Command::new(path).output().map_err(|e| e.to_string())?;
    if !out.status.success() {
        return Err("clinfo failed".into());
    }
    let s = String::from_utf8_lossy(&out.stdout);
    let mut list = Vec::new();
    let mut platform = None;
    let mut vendor = None;
    let mut version = None;
    for line in s.lines() {
        let l = line.trim();
        if l.starts_with("Platform Name") && platform.is_none() {
            platform = l.split(':').nth(1).map(|v| v.trim().to_string());
        } else if l.starts_with("Platform Vendor") && vendor.is_none() {
            vendor = l.split(':').nth(1).map(|v| v.trim().to_string());
        } else if l.starts_with("Platform Version") && version.is_none() {
            version = l.split(':').nth(1).map(|v| v.trim().to_string());
        }
        if platform.is_some() && vendor.is_some() && version.is_some() {
            break;
        }
    }

    if let (Some(p), Some(v), Some(ver)) = (platform, vendor, version) {
        list.push(GpuSpec {
            vendor: if v.to_lowercase().contains("nvidia") {
                GpuVendor::Nvidia
            } else if v.to_lowercase().contains("advanced micro devices")
                || v.to_lowercase().contains("amd")
            {
                GpuVendor::AMD
            } else if v.to_lowercase().contains("intel") {
                GpuVendor::Intel
            } else {
                GpuVendor::Other
            },
            model: None,
            vram_mb: None,
            bus: None,
            cuda: None,
            rocm: None,
            opencl: Some(OpenClDeviceSpec {
                platform: p,
                vendor: v,
                version: ver,
            }),
        });
    }
    Ok(list)
}

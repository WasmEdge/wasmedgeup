use crate::system::spec::{
    AcceleratorSupport, CudaSpec, GpuSpec, GpuVendor, OpenClDeviceSpec, RocmSpec,
};
use std::path::PathBuf;
use std::process::Command;

pub fn detect_gpu() -> (Vec<GpuSpec>, AcceleratorSupport, Vec<String>, Vec<String>) {
    let notes = Vec::new();
    let mut errors = Vec::new();

    let nvidia_smi = which("nvidia-smi");
    let rocminfo = which("rocminfo");
    let clinfo = which("clinfo");
    let vulkaninfo = which("vulkaninfo");

    let mut gpus: Vec<GpuSpec> = Vec::new();

    if let Some(path) = nvidia_smi.clone() {
        match query_nvidia_smi(&path) {
            Ok(mut list) => gpus.append(&mut list),
            Err(e) => errors.push(format!("nvidia-smi: {e}")),
        }
    }

    if let Some(path) = rocminfo.clone() {
        match query_rocminfo(&path) {
            Ok(mut list) => gpus.append(&mut list),
            Err(e) => errors.push(format!("rocminfo: {e}")),
        }
    }

    let opencl_available = clinfo.is_some();
    if let Some(path) = clinfo.clone() {
        if gpus.is_empty() {
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

    (gpus, accelerators, notes, errors)
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

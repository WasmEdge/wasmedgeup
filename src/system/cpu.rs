use crate::system::spec::{CpuClass, CpuFeature, CpuSpec};
use crate::target::TargetArch;
use std::collections::HashSet;

#[cfg(unix)]
use std::fs;
#[cfg(unix)]
use std::process::Command;

#[cfg(windows)]
use raw_cpuid::CpuId;
#[cfg(windows)]
use sysinfo::{CpuRefreshKind, RefreshKind, System};

type ProcCpuInfo = (
    Option<String>,      // vendor
    Option<String>,      // model
    Option<u32>,         // physical cores (sockets)
    Option<u32>,         // logical cores (cpus)
    HashSet<CpuFeature>, // feature flags
);

pub fn detect_cpu() -> (CpuSpec, Vec<String>, Vec<String>) {
    #[cfg(unix)]
    {
        let notes = Vec::new();
        let mut errors = Vec::new();

        let arch = TargetArch::default();

        let mut vendor = None;
        let mut model = None;
        let mut cores_physical = None;
        let mut cores_logical = None;
        let mut features: HashSet<CpuFeature> = HashSet::new();

        if let Ok((v, m, phys, logi, flags)) = parse_proc_cpuinfo() {
            vendor = v;
            model = m;
            cores_physical = phys;
            cores_logical = logi;
            features.extend(flags);
        } else if let Ok((phys, logi)) = parse_lscpu() {
            cores_physical = phys;
            cores_logical = logi;
        } else {
            errors.push("cpu: unable to parse /proc/cpuinfo or lscpu".to_string());
        }

        let class = classify(&arch, &features);

        let cpu = CpuSpec {
            arch,
            vendor,
            model,
            cores_physical,
            cores_logical,
            features,
            class,
        };

        (cpu, notes, errors)
    }

    #[cfg(windows)]
    {
        let notes = Vec::new();
        let errors = Vec::new();
        let arch = TargetArch::default();

        let cpuid = CpuId::new();
        let vendor = cpuid.get_vendor_info().map(|v| v.as_str().to_string());
        let model = cpuid
            .get_processor_brand_string()
            .map(|s| s.as_str().trim().to_string());

        let sys =
            System::new_with_specifics(RefreshKind::new().with_cpu(CpuRefreshKind::everything()));
        let cores_logical = Some(sys.cpus().len() as u32);
        let cores_physical = None;

        let mut features: HashSet<CpuFeature> = HashSet::new();
        if let Some(feat) = cpuid.get_feature_info() {
            if feat.has_sse2() {
                features.insert(CpuFeature::SSE2);
            }
            if feat.has_sse41() {
                features.insert(CpuFeature::SSE4_1);
            }
            if feat.has_sse42() {
                features.insert(CpuFeature::SSE4_2);
            }
            if feat.has_aesni() {
                features.insert(CpuFeature::AESNI);
            }
            if feat.has_popcnt() {
                features.insert(CpuFeature::POPCNT);
            }
        }
        if let Some(ext) = cpuid.get_extended_feature_info() {
            if ext.has_avx2() {
                features.insert(CpuFeature::AVX2);
            }
            if ext.has_bmi1() {
                features.insert(CpuFeature::BMI1);
            }
            if ext.has_bmi2() {
                features.insert(CpuFeature::BMI2);
            }
        }
        if let Some(info) = cpuid.get_feature_info() {
            if info.has_avx() {
                features.insert(CpuFeature::AVX);
            }
        }
        if let Some(leaf7) = cpuid.get_extended_feature_info() {
            if leaf7.has_avx512f() {
                features.insert(CpuFeature::AVX512);
            }
        }
        if cpuid
            .get_feature_info()
            .map(|f| f.has_fma())
            .unwrap_or(false)
        {
            features.insert(CpuFeature::FMA);
        }

        let class = classify(&arch, &features);
        let cpu = CpuSpec {
            arch,
            vendor,
            model,
            cores_physical,
            cores_logical,
            features,
            class,
        };
        (cpu, notes, errors)
    }
}

#[cfg(unix)]
fn parse_proc_cpuinfo() -> Result<ProcCpuInfo, String> {
    let content = fs::read_to_string("/proc/cpuinfo").map_err(|e| e.to_string())?;
    let mut vendor: Option<String> = None;
    let mut model: Option<String> = None;
    let mut logical_count: u32 = 0;
    let mut physical_ids: HashSet<String> = HashSet::new();
    let mut flags_set: HashSet<CpuFeature> = HashSet::new();

    for block in content.split("\n\n") {
        if block.trim().is_empty() {
            continue;
        }
        logical_count += 1;
        for line in block.lines() {
            if let Some((k, v)) = line.split_once(':') {
                let key = k.trim();
                let val = v.trim();
                match key {
                    "vendor_id" | "CPU implementer" => {
                        if vendor.is_none() {
                            vendor = Some(val.to_string());
                        }
                    }
                    "model name" | "Hardware" => {
                        if model.is_none() {
                            model = Some(val.to_string());
                        }
                    }
                    "physical id" => {
                        physical_ids.insert(val.to_string());
                    }
                    "flags" | "Features" => {
                        flags_set.extend(parse_flags(val));
                    }
                    _ => {}
                }
            }
        }
    }

    let phys = if !physical_ids.is_empty() {
        Some(physical_ids.len() as u32)
    } else {
        None
    };
    let logi = if logical_count > 0 {
        Some(logical_count)
    } else {
        None
    };
    Ok((vendor, model, phys, logi, flags_set))
}

#[cfg(unix)]
fn parse_lscpu() -> Result<(Option<u32>, Option<u32>), String> {
    let out = Command::new("lscpu").output().map_err(|e| e.to_string())?;
    if !out.status.success() {
        return Err("lscpu failed".into());
    }
    let s = String::from_utf8_lossy(&out.stdout);
    let mut phys = None;
    let mut logi = None;
    for line in s.lines() {
        if let Some((k, v)) = line.split_once(':') {
            let k = k.trim().to_lowercase();
            let v = v.trim();
            if k == "socket(s)" || k == "sockets" {
                phys = v.parse().ok();
            }
            if k == "cpu(s)" {
                logi = v.parse().ok();
            }
        }
    }
    Ok((phys, logi))
}

pub fn parse_flags(s: &str) -> HashSet<CpuFeature> {
    let mut set = HashSet::new();
    for f in s.split_whitespace() {
        match f.to_lowercase().as_str() {
            // x86
            "sse2" => {
                set.insert(CpuFeature::SSE2);
            }
            "sse4_1" => {
                set.insert(CpuFeature::SSE4_1);
            }
            "sse4_2" => {
                set.insert(CpuFeature::SSE4_2);
            }
            "avx" => {
                set.insert(CpuFeature::AVX);
            }
            "avx2" => {
                set.insert(CpuFeature::AVX2);
            }
            f if f.starts_with("avx512") => {
                set.insert(CpuFeature::AVX512);
            }
            "fma" => {
                set.insert(CpuFeature::FMA);
            }
            "bmi1" => {
                set.insert(CpuFeature::BMI1);
            }
            "bmi2" => {
                set.insert(CpuFeature::BMI2);
            }
            "aes" | "aesni" => {
                set.insert(CpuFeature::AESNI);
            }
            "popcnt" => {
                set.insert(CpuFeature::POPCNT);
            }
            // arm
            "neon" | "asimd" => {
                set.insert(CpuFeature::NEON);
            }
            "sve" => {
                set.insert(CpuFeature::SVE);
            }
            "sve2" => {
                set.insert(CpuFeature::SVE2);
            }
            _ => {}
        }
    }
    set
}

pub fn classify(arch: &TargetArch, features: &HashSet<CpuFeature>) -> CpuClass {
    match arch {
        TargetArch::X86_64 => {
            if features.contains(&CpuFeature::AVX512) {
                CpuClass::Avx512
            } else if features.contains(&CpuFeature::AVX2) {
                CpuClass::Avx2
            } else if features.contains(&CpuFeature::AVX) {
                CpuClass::Avx
            } else {
                CpuClass::NoAvx
            }
        }
        TargetArch::Aarch64 => {
            if features.contains(&CpuFeature::SVE2) {
                CpuClass::Sve2
            } else if features.contains(&CpuFeature::SVE) {
                CpuClass::Sve
            } else if features.contains(&CpuFeature::NEON) {
                CpuClass::Neon
            } else {
                CpuClass::Generic
            }
        }
    }
}

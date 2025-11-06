use wasmedgeup::system;
use wasmedgeup::system::cpu::{classify, parse_flags};
use wasmedgeup::system::plugins::platform_key_from_specs;
use wasmedgeup::system::spec::{CpuClass, CpuFeature};
use wasmedgeup::target::TargetArch;

#[test]
fn test_platform_key_detect_non_empty() {
    let spec = system::detect();
    let key = platform_key_from_specs(&spec.os).expect("platform key");
    assert!(!key.is_empty());
}

#[test]
fn test_platform_key_has_known_arch_suffix() {
    let spec = system::detect();
    let key = platform_key_from_specs(&spec.os).expect("platform key");
    assert!(
        key.ends_with("x86_64") || key.ends_with("aarch64") || key.ends_with("arm64"),
        "unexpected platform key suffix: {key}"
    );
}

#[test]
fn test_platform_key_prefix_is_reasonable() {
    let spec = system::detect();
    let key = platform_key_from_specs(&spec.os).expect("platform key");
    let ok_prefix = key.starts_with("ubuntu20_04_")
        || key.starts_with("ubuntu22_04_")
        || key.starts_with("manylinux2014_")
        || key.starts_with("manylinux_2_28_")
        || key.starts_with("darwin_")
        || key.starts_with("windows_");
    assert!(ok_prefix, "unexpected platform key prefix: {key}");
}

#[test]
fn test_cpu_parse_flags_x86() {
    let flags = parse_flags("sse2 sse4_1 sse4_2 avx avx2 avx512f fma bmi1 bmi2 aes popcnt");
    assert!(flags.contains(&CpuFeature::SSE2));
    assert!(flags.contains(&CpuFeature::SSE4_1));
    assert!(flags.contains(&CpuFeature::SSE4_2));
    assert!(flags.contains(&CpuFeature::AVX));
    assert!(flags.contains(&CpuFeature::AVX2));
    assert!(flags.contains(&CpuFeature::AVX512));
    assert!(flags.contains(&CpuFeature::FMA));
    assert!(flags.contains(&CpuFeature::BMI1));
    assert!(flags.contains(&CpuFeature::BMI2));
    assert!(flags.contains(&CpuFeature::AESNI));
    assert!(flags.contains(&CpuFeature::POPCNT));
}

#[test]
fn test_detect_cpu_smoke_has_reasonable_values() {
    let spec = system::detect();

    if cfg!(target_os = "macos") {
        assert!(matches!(
            spec.cpu.arch,
            TargetArch::X86_64 | TargetArch::Aarch64
        ));
    } else {
        assert!(spec.cpu.cores_logical.is_some() || !spec.cpu.features.is_empty());
    }
}

#[test]
fn test_cpu_classify_direct_x86() {
    let flags = parse_flags("sse2 sse4_2 avx avx2");
    let class = classify(&TargetArch::X86_64, &flags);

    match class {
        CpuClass::Avx512 | CpuClass::Avx2 | CpuClass::Avx => {}
        other => panic!("unexpected x86 class for given flags: {:?}", other),
    }
}

#[test]
fn test_cpu_classify_direct_arm() {
    let flags = parse_flags("neon");
    let class = classify(&TargetArch::Aarch64, &flags);

    match class {
        CpuClass::Sve2 | CpuClass::Sve | CpuClass::Neon | CpuClass::NeonOnly => {}
        other => panic!("unexpected ARM class for given flags: {:?}", other),
    }
}

use wasmedgeup::system::cpu::{classify, parse_flags};
use wasmedgeup::system::plugins::{platform_key_from_specs, plugin_platform_key};
use wasmedgeup::system::{self, CpuClass, CpuFeature, LibcKind, LibcSpec, OsSpec};
use wasmedgeup::target::{TargetArch, TargetOS};

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

#[test]
fn test_plugin_platform_key_linux_manylinux_switch() {
    let os = OsSpec {
        os_type: TargetOS::Linux,
        arch: TargetArch::X86_64,
        distro: Some("ubuntu".to_string()),
        version: Some("22.04".to_string()),
        kernel: Some("6.4.0".to_string()),
        libc: LibcSpec {
            kind: LibcKind::Glibc,
            version: Some("2.35".to_string()),
        },
    };

    let v014 = semver::Version::parse("0.14.0").unwrap();
    let v015 = semver::Version::parse("0.15.0").unwrap();

    let key_014 = plugin_platform_key(&os, &v014).expect("key for 0.14.x");
    let key_015 = plugin_platform_key(&os, &v015).expect("key for 0.15.x");

    assert_eq!(key_014, "manylinux2014_x86_64");
    assert_eq!(key_015, "manylinux_2_28_x86_64");
}

#[test]
fn test_plugin_platform_key_darwin_major() {
    let mut os = OsSpec {
        os_type: TargetOS::Darwin,
        arch: TargetArch::Aarch64,
        distro: None,
        version: Some("23.4.0".to_string()),
        kernel: None,
        libc: LibcSpec {
            kind: LibcKind::Glibc,
            version: None,
        },
    };

    let v = semver::Version::parse("0.15.0").unwrap();
    let key_arm = plugin_platform_key(&os, &v).expect("darwin arm64 key");
    assert_eq!(key_arm, "darwin_23-arm64");

    os.arch = TargetArch::X86_64;
    let key_x64 = plugin_platform_key(&os, &v).expect("darwin x86_64 key");
    assert_eq!(key_x64, "darwin_23-x86_64");

    // Fallback when no version present
    os.version = None;
    let key_fallback = plugin_platform_key(&os, &v).expect("darwin generic key");
    assert_eq!(key_fallback, "darwin_x86_64");
}

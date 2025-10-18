use wasmedgeup::system;
use wasmedgeup::system::plugins::platform_key_from_specs;

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

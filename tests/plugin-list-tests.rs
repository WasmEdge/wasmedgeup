use serde_json::Value;
use wasmedgeup::{
    api::runtime_ge_015,
    commands::plugin::list::platform_fallbacks,
    system::{self, plugins::platform_key_from_specs},
};

const ASSET_PREFIX: &str = "WasmEdge-plugin-";
const GH_RELEASE_TAG_API: &str = "https://api.github.com/repos/WasmEdge/WasmEdge/releases/tags";

#[test]
fn test_runtime_ge_015_cases() {
    assert!(!runtime_ge_015("0.14.2"));
    assert!(runtime_ge_015("0.15.0"));
    assert!(runtime_ge_015("0.15.1"));
    assert!(runtime_ge_015("1.0.0"));
    assert!(runtime_ge_015("not-a-version"));
    assert!(runtime_ge_015(""));
}

#[test]
fn test_platform_fallbacks_ubuntu20_old_runtime() {
    let out = platform_fallbacks("ubuntu20_04_x86_64", "0.14.2");
    assert!(out.contains(&"ubuntu20_04_x86_64".to_string()));
    assert!(out.contains(&"manylinux2014_x86_64".to_string()));
    assert!(!out.contains(&"manylinux_2_28_x86_64".to_string()));
}

#[test]
fn test_platform_fallbacks_ubuntu20_new_runtime() {
    let out = platform_fallbacks("ubuntu20_04_x86_64", "0.15.0");
    assert!(out.contains(&"ubuntu20_04_x86_64".to_string()));
    assert!(out.contains(&"manylinux_2_28_x86_64".to_string()));
}

#[test]
fn test_platform_fallbacks_ubuntu22_any_runtime() {
    let out = platform_fallbacks("ubuntu22_04_x86_64", "0.14.2");
    assert!(out.contains(&"ubuntu22_04_x86_64".to_string()));
    assert!(out.contains(&"manylinux_2_28_x86_64".to_string()));
}

#[test]
fn test_platform_fallbacks_manylinux2014_with_new_runtime() {
    let out = platform_fallbacks("manylinux2014_x86_64", "0.16.0");
    assert!(out.contains(&"manylinux2014_x86_64".to_string()));
    assert!(out.contains(&"manylinux_2_28_x86_64".to_string()));
}

#[tokio::test]
async fn test_github_assets_list_contains_expected_platform() {
    let spec = system::detect();
    let platform = platform_key_from_specs(&spec.os).expect("platform key");
    let runtime = match system::toolchain::get_installed_wasmedge_version() {
        Ok(v) => v,
        Err(_) => "0.15.0".to_string(),
    };
    let url = format!("{GH_RELEASE_TAG_API}/{runtime}");

    let client = reqwest::Client::new();
    let resp = client
        .get(&url)
        .header("User-Agent", "wasmedgeup-tests")
        .send()
        .await;

    let resp = match resp {
        Ok(r) if r.status().is_success() => r,
        _ => return,
    };
    let text = resp.text().await.unwrap_or_default();
    let v: Value = match serde_json::from_str(&text) {
        Ok(j) => j,
        Err(_) => return,
    };

    let mut names: Vec<String> = Vec::new();
    if let Some(arr) = v.get("assets").and_then(|a| a.as_array()) {
        for a in arr {
            if let Some(name) = a.get("name").and_then(|s| s.as_str()) {
                if name.starts_with(ASSET_PREFIX) {
                    names.push(name.to_string());
                }
            }
        }
    }

    let candidates = platform_fallbacks(&platform, &runtime);
    let mut matched = false;
    for plat in &candidates {
        if names.iter().any(|n| n.contains(plat)) {
            matched = true;
            break;
        }
    }
    assert!(
        matched || names.is_empty(),
        "Either matched a platform asset, or release has no plugin assets at all"
    );
}

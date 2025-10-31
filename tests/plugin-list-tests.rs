use serde_json::Value;
use wasmedgeup::{system, system::plugins::platform_key_from_specs};

const ASSET_PREFIX: &str = "WasmEdge-plugin-";
const GH_RELEASE_TAG_API: &str = "https://api.github.com/repos/WasmEdge/WasmEdge/releases/tags";

fn platform_fallbacks_for_test(primary: &str, runtime: &str) -> Vec<String> {
    fn runtime_ge_015(runtime: &str) -> bool {
        semver::Version::parse(runtime)
            .map(|v| v >= semver::Version::new(0, 15, 0))
            .unwrap_or(true)
    }
    const UBUNTU20_PREFIX: &str = "ubuntu20_04_";
    const UBUNTU22_PREFIX: &str = "ubuntu22_04_";
    const MANYLINUX2014_PREFIX: &str = "manylinux2014_";
    const MANYLINUX_2_28_PREFIX: &str = "manylinux_2_28_";

    let rt_ge_015 = runtime_ge_015(runtime);
    let mut out = vec![primary.to_string()];
    if primary.starts_with(UBUNTU20_PREFIX) {
        if rt_ge_015 {
            out.push(primary.replacen(UBUNTU20_PREFIX, MANYLINUX_2_28_PREFIX, 1));
        } else {
            out.push(primary.replacen(UBUNTU20_PREFIX, MANYLINUX2014_PREFIX, 1));
        }
    } else if primary.starts_with(UBUNTU22_PREFIX) {
        out.push(primary.replacen(UBUNTU22_PREFIX, MANYLINUX_2_28_PREFIX, 1));
    } else if primary.starts_with(MANYLINUX2014_PREFIX) && rt_ge_015 {
        out.push(primary.replacen(MANYLINUX2014_PREFIX, MANYLINUX_2_28_PREFIX, 1));
    }
    out.sort();
    out.dedup();
    out
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

    let candidates = platform_fallbacks_for_test(&platform, &runtime);
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

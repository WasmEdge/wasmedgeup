use crate::api::runtime_ge_015;
use crate::cli::{CommandContext, CommandExecutor};
use crate::constants::{WASMEDGE_GH_RELEASE_TAG_API, WASMEDGE_RELEASE_BASE_URL};
use crate::prelude::*;
use crate::system;
use crate::system::plugins::plugin_platform_key;
use clap::Args;
use serde_json::Value;
use std::cmp::Ordering;
use std::collections::HashSet;

const UA: &str = "wasmedgeup";
const ASSET_PREFIX: &str = "WasmEdge-plugin-";
const TAR_GZ: &str = ".tar.gz";
const ZIP: &str = ".zip";

const UBUNTU20_PREFIX: &str = "ubuntu20_04_";
const UBUNTU22_PREFIX: &str = "ubuntu22_04_";
const MANYLINUX2014_PREFIX: &str = "manylinux2014_";
const MANYLINUX_2_28_PREFIX: &str = "manylinux_2_28_";

#[derive(Debug, Args)]
pub struct PluginListArgs {
    /// Show all (including assets that are not found for this runtime/platform)
    #[arg(long)]
    all: bool,

    /// Override the WasmEdge runtime version to check (e.g., 0.15.0)
    #[arg(long)]
    runtime: Option<String>,

    /// Filter by a single plugin name
    #[arg(long)]
    name: Option<String>,
}

impl CommandExecutor for PluginListArgs {
    async fn execute(self, _ctx: CommandContext) -> Result<()> {
        let spec = system::detect();

        // Short-circuit on `--runtime`: when the user passes one explicitly we
        // must not spawn `wasmedge --version`, otherwise an explicit-runtime
        // call still pays for (and depends on) local toolchain detection.
        let runtime = if let Some(r) = self.runtime {
            r
        } else {
            match system::toolchain::get_installed_wasmedge_version() {
                Some(v) => v,
                None => {
                    eprintln!(
                        "WasmEdge runtime not found. Install a runtime first \
                        (e.g., wasmedgeup install 0.15.0)."
                    );
                    return Err(Error::RuntimeNotFound);
                }
            }
        };

        let platform = match semver::Version::parse(&runtime) {
            Ok(v) => match plugin_platform_key(&spec.os, &v) {
                Ok(p) => p,
                Err(e) => {
                    eprintln!("{e}");
                    return Err(e);
                }
            },
            Err(e) => {
                eprintln!("Invalid runtime version '{runtime}' (expected semver like 0.15.0)");
                return Err(Error::SemVer { source: e });
            }
        };

        let cuda_hint = spec.accelerators.cuda_available;
        let noavx_hint = matches!(spec.cpu.class, crate::system::spec::CpuClass::NoAvx)
            || !spec
                .cpu
                .features
                .contains(&crate::system::spec::CpuFeature::AVX);

        let assets = match fetch_release_assets(&runtime).await {
            Ok(v) => v,
            Err(e) => {
                tracing::warn!(error = %e, tag = %runtime, "failed to fetch plugin release assets");
                eprintln!("failed to fetch release assets for tag {runtime}: {e}");
                Vec::new()
            }
        };

        let mut name_set: HashSet<String> = HashSet::new();
        for a in &assets {
            if a.version == runtime {
                name_set.insert(a.plugin.clone());
            }
        }

        let mut candidates: Vec<String> = name_set.into_iter().collect();

        if let Some(filter) = &self.name {
            candidates.retain(|p| p == filter);
        }

        candidates.sort_by(|a, b| order_plugins(a, b, cuda_hint, noavx_hint));

        let platform_candidates = platform_fallbacks(&platform, &runtime);
        let mut rows: Vec<Row> = Vec::new();

        for a in &assets {
            if a.version != runtime {
                continue;
            }
            if !platform_candidates.iter().any(|p| p == &a.platform) {
                continue;
            }
            if let Some(filter) = &self.name {
                if &a.plugin != filter {
                    continue;
                }
            }
            rows.push(Row {
                name: a.plugin.clone(),
                version: a.version.clone(),
                status: "available".to_string(),
            });
        }

        if rows.is_empty() && self.all {
            for name in &candidates {
                let probes = if name == "wasi_nn-ggml" {
                    if cuda_hint {
                        vec!["wasi_nn-ggml-cuda", "wasi_nn-ggml"]
                    } else if noavx_hint {
                        vec!["wasi_nn-ggml-noavx", "wasi_nn-ggml"]
                    } else {
                        vec!["wasi_nn-ggml"]
                    }
                } else {
                    vec![name.as_str()]
                };
                for probe in probes {
                    for plat in &platform_candidates {
                        let url_targz = format!(
                            "{WASMEDGE_RELEASE_BASE_URL}/{runtime}/{ASSET_PREFIX}{probe}-{runtime}-{plat}{TAR_GZ}"
                        );
                        let url_zip = format!(
                            "{WASMEDGE_RELEASE_BASE_URL}/{runtime}/{ASSET_PREFIX}{probe}-{runtime}-{plat}{ZIP}"
                        );
                        let available = head_ok(&url_targz).await || head_ok(&url_zip).await;
                        rows.push(Row {
                            name: probe.to_string(),
                            version: runtime.clone(),
                            status: if available {
                                format!("available ({plat})")
                            } else {
                                format!("not found ({plat})")
                            },
                        });
                    }
                }
            }
        }

        rows.sort_by(|a, b| match a.name.cmp(&b.name) {
            Ordering::Equal => version_desc(&a.version, &b.version),
            other => other,
        });

        println!("Runtime: {runtime}\nPlatform: {platform}");
        if rows.is_empty() {
            println!(
                "\nNo plugins {} for this runtime/platform.",
                if self.all {
                    "(including missing entries)"
                } else {
                    "found"
                }
            );
            return Ok(());
        }
        let name_w = 28usize;
        let ver_w = 12usize;
        println!(
            "\n{:<name_w$} {:<ver_w$} STATUS",
            "PLUGIN",
            "VERSION",
            name_w = name_w,
            ver_w = ver_w
        );
        println!(
            "{} {} {}",
            "-".repeat(name_w),
            "-".repeat(ver_w),
            "-".repeat(40)
        );
        for r in rows {
            println!(
                "{:<name_w$} {:<ver_w$} {}",
                r.name,
                r.version,
                r.status,
                name_w = name_w,
                ver_w = ver_w,
            );
        }

        Ok(())
    }
}

#[derive(Debug)]
struct Row {
    name: String,
    version: String,
    status: String,
}

fn version_desc(a: &str, b: &str) -> Ordering {
    match (semver::Version::parse(a), semver::Version::parse(b)) {
        (Ok(va), Ok(vb)) => vb.cmp(&va),
        _ => b.cmp(a),
    }
}

fn order_plugins(a: &str, b: &str, cuda: bool, noavx: bool) -> Ordering {
    let rank = |name: &str| -> i32 {
        if cuda && name.contains("ggml-cuda") {
            return 0;
        }
        if noavx && name.contains("ggml-noavx") {
            return 1;
        }
        if name.contains("ggml") {
            return 2;
        }
        3
    };
    rank(a).cmp(&rank(b)).then(a.cmp(b))
}

async fn head_ok(url: &str) -> bool {
    let client = reqwest::Client::new();
    if let Ok(resp) = client.head(url).send().await {
        if resp.status().is_success() {
            return true;
        }
    }
    if let Ok(resp) = client.get(url).send().await {
        return resp.status().is_success();
    }
    false
}

#[derive(Debug, Clone)]
struct AssetInfo {
    plugin: String,
    version: String,
    platform: String,
}

async fn fetch_release_assets(tag: &str) -> Result<Vec<AssetInfo>> {
    use snafu::ResultExt;

    let url = format!("{WASMEDGE_GH_RELEASE_TAG_API}/{tag}");
    let client = reqwest::Client::new();
    let resp = client
        .get(&url)
        .header("User-Agent", UA)
        .send()
        .await
        .context(RequestSnafu {
            resource: "plugin release metadata",
        })?;
    // A 404 means the tag doesn't exist (or has no published assets) —
    // degrade gracefully to an empty list. Other non-2xx statuses (403
    // rate-limit, 5xx outage, etc.) propagate as errors so the caller can
    // surface them to the user instead of silently reporting "no plugins".
    if resp.status() == reqwest::StatusCode::NOT_FOUND {
        tracing::debug!(tag, "release metadata 404 — tag has no published assets");
        return Ok(Vec::new());
    }
    let resp = resp.error_for_status().context(RequestSnafu {
        resource: "plugin release metadata",
    })?;
    let text = resp.text().await.context(RequestSnafu {
        resource: "plugin release metadata body",
    })?;
    let v: Value = serde_json::from_str(&text).context(JsonSnafu {
        resource: "plugin release metadata",
    })?;
    let mut out = Vec::new();
    if let Some(arr) = v.get("assets").and_then(|a| a.as_array()) {
        for a in arr {
            let name = a.get("name").and_then(|s| s.as_str()).unwrap_or("");
            if !name.starts_with(ASSET_PREFIX) {
                continue;
            }
            if let Some(info) = parse_asset_name(name, tag) {
                out.push(AssetInfo {
                    plugin: info.0,
                    version: info.1,
                    platform: info.2,
                });
            }
        }
    }
    Ok(out)
}

fn parse_asset_name(name: &str, tag: &str) -> Option<(String, String, String)> {
    let rest = name.strip_prefix(ASSET_PREFIX)?;
    let needle = format!("-{tag}-");
    if let Some(idx) = rest.find(&needle) {
        let plugin = &rest[..idx];
        let plat_with_ext = &rest[idx + needle.len()..];
        let platform = plat_with_ext
            .strip_suffix(TAR_GZ)
            .or_else(|| plat_with_ext.strip_suffix(ZIP))
            .unwrap_or(plat_with_ext);
        return Some((plugin.to_string(), tag.to_string(), platform.to_string()));
    }
    None
}

pub fn platform_fallbacks(primary: &str, runtime: &str) -> Vec<String> {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_asset_name_targz() {
        let parsed = parse_asset_name(
            "WasmEdge-plugin-wasi_nn-ggml-0.15.0-manylinux_2_28_x86_64.tar.gz",
            "0.15.0",
        );
        assert_eq!(
            parsed,
            Some((
                "wasi_nn-ggml".to_string(),
                "0.15.0".to_string(),
                "manylinux_2_28_x86_64".to_string(),
            ))
        );
    }

    #[test]
    fn parse_asset_name_zip() {
        let parsed = parse_asset_name(
            "WasmEdge-plugin-wasi_crypto-0.14.1-windows_x86_64.zip",
            "0.14.1",
        );
        assert_eq!(
            parsed,
            Some((
                "wasi_crypto".to_string(),
                "0.14.1".to_string(),
                "windows_x86_64".to_string(),
            ))
        );
    }

    #[test]
    fn parse_asset_name_rejects_unrelated_prefix() {
        let parsed = parse_asset_name("WasmEdge-0.15.0-linux.tar.gz", "0.15.0");
        assert_eq!(parsed, None);
    }

    #[test]
    fn parse_asset_name_rejects_tag_mismatch() {
        // The needle "-0.15.0-" is not present in an archive tagged 0.14.1.
        let parsed = parse_asset_name(
            "WasmEdge-plugin-wasi_nn-ggml-0.14.1-manylinux2014_x86_64.tar.gz",
            "0.15.0",
        );
        assert_eq!(parsed, None);
    }

    #[test]
    fn parse_asset_name_plugin_name_may_contain_dashes() {
        let parsed = parse_asset_name(
            "WasmEdge-plugin-wasi-logging-0.15.0-darwin_arm64.tar.gz",
            "0.15.0",
        );
        assert_eq!(
            parsed,
            Some((
                "wasi-logging".to_string(),
                "0.15.0".to_string(),
                "darwin_arm64".to_string(),
            ))
        );
    }
}

use crate::api::{plugin_asset_url, PluginAssetInfo, WasmEdgeApiClient};
use crate::cli::{CommandContext, CommandExecutor};
use crate::prelude::*;
use crate::system;
use crate::system::plugins::plugin_platform_key;
use crate::system::spec::{CpuClass, CpuFeature, SystemSpec};
use clap::Args;
use std::cmp::Ordering;
use std::collections::HashSet;

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

impl PluginListArgs {
    /// Pick the runtime tag: explicit `--runtime` wins; otherwise we ask
    /// `wasmedge --version` on PATH. Returns `RuntimeNotFound` if neither
    /// source yields a value so the caller can surface an install hint.
    ///
    /// Takes `runtime_arg` by value to preserve the short-circuit
    /// behaviour PR #265 fixed: passing `--runtime` must not pay for a
    /// `wasmedge --version` subprocess spawn.
    fn resolve_runtime_tag(runtime_arg: Option<String>) -> Result<String> {
        if let Some(r) = runtime_arg {
            return Ok(r);
        }
        match system::toolchain::get_installed_wasmedge_version() {
            Some(v) => Ok(v),
            None => {
                eprintln!(
                    "WasmEdge runtime not found. Install a runtime first \
                    (e.g., wasmedgeup install 0.15.0)."
                );
                Err(Error::RuntimeNotFound)
            }
        }
    }
}

impl CommandExecutor for PluginListArgs {
    async fn execute(self, ctx: CommandContext) -> Result<()> {
        let spec = system::detect();
        let runtime = Self::resolve_runtime_tag(self.runtime)?;
        let platform = resolve_platform_key(&runtime, &spec)?;

        let hints = PluginHints::from_spec(&spec);
        let assets = fetch_release_assets_or_warn(&ctx.client, &runtime).await;
        let candidates = collect_plugin_candidates(&assets, &runtime, &hints, self.name.as_deref());
        let platform_candidates = platform_fallbacks(&platform, &runtime);

        let mut rows = build_direct_rows(
            &assets,
            &runtime,
            &platform_candidates,
            self.name.as_deref(),
        );

        if rows.is_empty() && self.all {
            rows = build_probe_rows(
                &ctx.client,
                &candidates,
                &runtime,
                &platform_candidates,
                &hints,
            )
            .await?;
        }

        rows.sort_by(|a, b| match a.name.cmp(&b.name) {
            Ordering::Equal => version_desc(&a.version, &b.version),
            other => other,
        });

        print_plugin_table(&rows, &runtime, &platform, self.all);
        Ok(())
    }
}

#[derive(Debug)]
struct Row {
    name: String,
    version: String,
    status: String,
}

/// Host hints that bias plugin ordering: the CUDA-first / noavx-preferred
/// heuristics only make sense in context of the machine we're running on.
struct PluginHints {
    cuda: bool,
    noavx: bool,
}

impl PluginHints {
    fn from_spec(spec: &SystemSpec) -> Self {
        let cuda = spec.accelerators.cuda_available;
        let noavx = matches!(spec.cpu.class, CpuClass::NoAvx)
            || !spec.cpu.features.contains(&CpuFeature::AVX);
        Self { cuda, noavx }
    }
}

/// Parse `runtime` as semver and compute the platform key for plugin
/// archives; both failures print a user-facing message before returning.
fn resolve_platform_key(runtime: &str, spec: &SystemSpec) -> Result<String> {
    let v = semver::Version::parse(runtime).map_err(|source| {
        eprintln!("Invalid runtime version '{runtime}' (expected semver like 0.15.0)");
        Error::SemVer { source }
    })?;
    plugin_platform_key(&spec.os, &v).inspect_err(|e| eprintln!("{e}"))
}

/// Query GitHub's releases API; a failure is logged but not propagated —
/// the command degrades gracefully to an empty list in that case.
async fn fetch_release_assets_or_warn(
    client: &WasmEdgeApiClient,
    runtime: &str,
) -> Vec<PluginAssetInfo> {
    match client.github_release_assets(runtime).await {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!(error = %e, tag = %runtime, "failed to fetch plugin release assets");
            eprintln!("failed to fetch release assets for tag {runtime}: {e}");
            Vec::new()
        }
    }
}

/// Distinct plugin names present in `assets` for `runtime`, filtered by
/// `name_filter` when set, then sorted by [`order_plugins`] which biases
/// toward hardware-appropriate ggml variants.
fn collect_plugin_candidates(
    assets: &[PluginAssetInfo],
    runtime: &str,
    hints: &PluginHints,
    name_filter: Option<&str>,
) -> Vec<String> {
    let mut name_set: HashSet<String> = HashSet::new();
    for a in assets {
        if a.version == runtime {
            name_set.insert(a.plugin.clone());
        }
    }
    let mut candidates: Vec<String> = name_set.into_iter().collect();
    if let Some(filter) = name_filter {
        candidates.retain(|p| p == filter);
    }
    candidates.sort_by(|a, b| order_plugins(a, b, hints.cuda, hints.noavx));
    candidates
}

/// Rows derived directly from GitHub asset metadata — no network
/// speculation. An asset qualifies when its tag matches `runtime`, its
/// platform appears in `platform_candidates`, and it passes the name filter.
/// Each plugin and version yields at most one row, taken from the first
/// platform in `platform_candidates` that publishes it.
fn build_direct_rows(
    assets: &[PluginAssetInfo],
    runtime: &str,
    platform_candidates: &[String],
    name_filter: Option<&str>,
) -> Vec<Row> {
    let mut seen: HashSet<(&str, &str)> = HashSet::new();
    let mut rows: Vec<Row> = Vec::new();
    for platform in platform_candidates {
        for a in assets
            .iter()
            .filter(|a| a.version == runtime)
            .filter(|a| &a.platform == platform)
            .filter(|a| match name_filter {
                Some(filter) => a.plugin == filter,
                None => true,
            })
        {
            if seen.insert((a.plugin.as_str(), a.version.as_str())) {
                rows.push(Row {
                    name: a.plugin.clone(),
                    version: a.version.clone(),
                    status: "available".to_string(),
                });
            }
        }
    }
    rows
}

/// When `--all` is set and no direct rows exist, probe speculative URLs
/// (tar.gz + zip across every platform fallback) per candidate so the
/// user sees which combinations exist and which don't.
async fn build_probe_rows(
    client: &WasmEdgeApiClient,
    candidates: &[String],
    runtime: &str,
    platform_candidates: &[String],
    hints: &PluginHints,
) -> Result<Vec<Row>> {
    let mut rows: Vec<Row> = Vec::new();
    for name in candidates {
        for probe in probes_for(name, hints) {
            for plat in platform_candidates {
                let url_targz = plugin_asset_url(probe, runtime, plat, false)?;
                let url_zip = plugin_asset_url(probe, runtime, plat, true)?;
                let available = client.head_ok(url_targz).await || client.head_ok(url_zip).await;
                rows.push(Row {
                    name: probe.to_string(),
                    version: runtime.to_string(),
                    status: if available {
                        format!("available ({plat})")
                    } else {
                        format!("not found ({plat})")
                    },
                });
            }
        }
    }
    Ok(rows)
}

/// Which name variants to try for a given plugin base name. For the ggml
/// plugin we prefer cuda or noavx variants when the host hints indicate
/// they'd work better; other plugins just probe themselves.
fn probes_for<'a>(name: &'a str, hints: &PluginHints) -> Vec<&'a str> {
    if name == "wasi_nn-ggml" {
        if hints.cuda {
            vec!["wasi_nn-ggml-cuda", "wasi_nn-ggml"]
        } else if hints.noavx {
            vec!["wasi_nn-ggml-noavx", "wasi_nn-ggml"]
        } else {
            vec!["wasi_nn-ggml"]
        }
    } else {
        vec![name]
    }
}

fn print_plugin_table(rows: &[Row], runtime: &str, platform: &str, show_missing_hint: bool) {
    const NAME_W: usize = 28;
    const VER_W: usize = 12;
    const STATUS_W: usize = 40;

    println!("Runtime: {runtime}\nPlatform: {platform}");
    if rows.is_empty() {
        println!(
            "\nNo plugins {} for this runtime/platform.",
            if show_missing_hint {
                "(including missing entries)"
            } else {
                "found"
            }
        );
        return;
    }

    println!("\n{:<NAME_W$} {:<VER_W$} STATUS", "PLUGIN", "VERSION",);
    println!(
        "{} {} {}",
        "-".repeat(NAME_W),
        "-".repeat(VER_W),
        "-".repeat(STATUS_W),
    );
    for r in rows {
        println!("{:<NAME_W$} {:<VER_W$} {}", r.name, r.version, r.status,);
    }
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

/// Whether `runtime` publishes manylinux_2_28 Linux plugins. Uses the same
/// `0.15.0-rc.0` boundary as [`plugin_platform_key`], so 0.15.0 release
/// candidates count as 0.15. An unparseable runtime counts as 0.15 or newer.
fn runtime_uses_manylinux_2_28(runtime: &str) -> bool {
    match (
        semver::Version::parse(runtime),
        semver::Version::parse("0.15.0-rc.0"),
    ) {
        (Ok(version), Ok(boundary)) => version >= boundary,
        _ => true,
    }
}

pub fn platform_fallbacks(primary: &str, runtime: &str) -> Vec<String> {
    let use_manylinux_2_28 = runtime_uses_manylinux_2_28(runtime);
    let mut out = vec![primary.to_string()];
    if primary.starts_with(UBUNTU20_PREFIX) {
        if use_manylinux_2_28 {
            out.push(primary.replacen(UBUNTU20_PREFIX, MANYLINUX_2_28_PREFIX, 1));
        } else {
            out.push(primary.replacen(UBUNTU20_PREFIX, MANYLINUX2014_PREFIX, 1));
        }
    } else if primary.starts_with(UBUNTU22_PREFIX) {
        out.push(primary.replacen(UBUNTU22_PREFIX, MANYLINUX_2_28_PREFIX, 1));
    } else if primary.starts_with(MANYLINUX2014_PREFIX) && use_manylinux_2_28 {
        out.push(primary.replacen(MANYLINUX2014_PREFIX, MANYLINUX_2_28_PREFIX, 1));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn asset(plugin: &str, version: &str, platform: &str) -> PluginAssetInfo {
        PluginAssetInfo {
            plugin: plugin.to_string(),
            version: version.to_string(),
            platform: platform.to_string(),
        }
    }

    #[test]
    fn direct_rows_emit_one_row_per_plugin() {
        let assets = vec![
            asset("wasi_nn-ggml", "0.18.1", "manylinux_2_28_x86_64"),
            asset("wasi_nn-ggml", "0.18.1", "ubuntu20_04_x86_64"),
            asset("wasi_crypto", "0.18.1", "manylinux_2_28_x86_64"),
            asset("wasi_nn-openvino", "0.18.1", "ubuntu20_04_x86_64"),
            asset("wasi_nn-ggml", "0.18.1", "darwin_x86_64"),
            asset("wasi_logging", "0.18.0", "manylinux_2_28_x86_64"),
        ];
        let platforms = platform_fallbacks("ubuntu20_04_x86_64", "0.18.1");

        let names = |rows: Vec<Row>| -> Vec<String> { rows.into_iter().map(|r| r.name).collect() };
        let rows = build_direct_rows(&assets, "0.18.1", &platforms, Some("wasi_nn-ggml"));
        assert_eq!(names(rows), vec!["wasi_nn-ggml"]);

        let rows = build_direct_rows(&assets, "0.18.1", &platforms, None);
        assert!(rows.iter().all(|r| r.version == "0.18.1"));
        assert!(rows.iter().all(|r| r.status == "available"));
        assert_eq!(
            names(rows),
            vec!["wasi_nn-ggml", "wasi_nn-openvino", "wasi_crypto"]
        );
    }

    #[test]
    fn platform_fallbacks_count_release_candidates_as_015() {
        for (runtime, fallback) in [
            ("0.15.0-rc.0", "manylinux_2_28_aarch64"),
            ("0.15.0-rc.1", "manylinux_2_28_aarch64"),
            ("0.14.1", "manylinux2014_aarch64"),
            ("0.15.0", "manylinux_2_28_aarch64"),
            ("not-a-version", "manylinux_2_28_aarch64"),
        ] {
            assert_eq!(
                platform_fallbacks("ubuntu20_04_aarch64", runtime),
                vec!["ubuntu20_04_aarch64", fallback],
                "runtime {runtime}"
            );
        }
    }
}

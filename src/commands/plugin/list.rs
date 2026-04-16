use crate::api::{plugin_asset_url, runtime_ge_015};
use crate::cli::{CommandContext, CommandExecutor};
use crate::prelude::*;
use crate::system;
use crate::system::plugins::plugin_platform_key;
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

impl CommandExecutor for PluginListArgs {
    async fn execute(self, ctx: CommandContext) -> Result<()> {
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

        let assets = match ctx.client.github_release_assets(&runtime).await {
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
                        let url_targz = plugin_asset_url(probe, &runtime, plat, false)?;
                        let url_zip = plugin_asset_url(probe, &runtime, plat, true)?;
                        let available = ctx.client.head_ok(url_targz).await
                            || ctx.client.head_ok(url_zip).await;
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

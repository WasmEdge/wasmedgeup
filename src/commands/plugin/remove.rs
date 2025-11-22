use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};

use clap::Args;

use super::install::select_runtime_version;
use super::version::PluginVersion;
use crate::commands::default_path;
use crate::{
    cli::{CommandContext, CommandExecutor},
    error::{Error, Result},
};

#[derive(Debug, Args)]
pub struct PluginRemoveArgs {
    /// Names and versions of plugins to remove, e.g. `plugin1 plugin2@version`
    #[arg(value_parser = clap::value_parser!(PluginVersion))]
    pub plugins: Vec<PluginVersion>,

    /// Remove plugins from this runtime version (defaults to latest installed)
    #[arg(long, value_name = "RUNTIME_VERSION")]
    pub runtime: Option<String>,

    /// Set the install location for the WasmEdge runtime (defaults to $HOME/.wasmedge)
    #[arg(short, long)]
    pub path: Option<PathBuf>,
}

fn normalize_name(s: &str) -> String {
    s.chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .flat_map(|c| c.to_lowercase())
        .collect()
}

impl CommandExecutor for PluginRemoveArgs {
    #[tracing::instrument(name = "plugin.remove", skip_all, fields(plugins = ?self.plugins))]
    async fn execute(self, _ctx: CommandContext) -> Result<()> {
        if self.plugins.is_empty() {
            return Err(Error::NoPluginsSpecified);
        }

        let versions_dir = self
            .path
            .clone()
            .unwrap_or_else(default_path)
            .join("versions");

        let runtime_version = select_runtime_version(&versions_dir, self.runtime.as_deref())?;
        let version_dir = versions_dir.join(runtime_version.to_string());

        if !version_dir.exists() {
            return Err(Error::VersionNotFound {
                version: runtime_version.to_string(),
            });
        }

        let plugin_dir = version_dir.join("plugin");
        let stable_plugin_dir = versions_dir
            .parent()
            .unwrap_or(&versions_dir)
            .join("plugin");

        let mut by_name: BTreeMap<String, Vec<PathBuf>> = BTreeMap::new();

        let mut searched_dirs: Vec<PathBuf> = Vec::new();
        if plugin_dir.exists() {
            searched_dirs.push(plugin_dir.clone());
        }
        if stable_plugin_dir.exists() {
            searched_dirs.push(stable_plugin_dir.clone());
        }

        for dir in &searched_dirs {
            let mut rd = tokio::fs::read_dir(dir).await?;
            while let Some(entry) = rd.next_entry().await? {
                let path = entry.path();
                if !entry
                    .file_type()
                    .await
                    .map(|t| t.is_file())
                    .unwrap_or(false)
                {
                    continue;
                }
                if let Some(raw_name) = extract_plugin_name(&path) {
                    let norm = normalize_name(&raw_name);
                    by_name.entry(raw_name).or_default().push(path.clone());
                    by_name.entry(norm).or_default().push(path.clone());
                }
            }
        }

        if by_name.is_empty() {
            tracing::info!(
                dirs = ?searched_dirs,
                "No plugin files found to remove in any plugin directory"
            );
            return Ok(());
        }

        let mut requested: Vec<String> = Vec::new();
        for p in self.plugins {
            match p {
                PluginVersion::Name(n) => requested.push(n),
                PluginVersion::NameAndVersion(n, v) => {
                    tracing::warn!(
                        plugin = %n,
                        version = %v,
                        "Plugin remove does not track per-plugin version on disk; removing by name"
                    );
                    requested.push(n)
                }
            }
        }

        let mut removed_any = false;
        let mut removed_targets: HashSet<PathBuf> = HashSet::new();
        let mut missing: Vec<String> = Vec::new();
        for want in requested {
            let key_norm = normalize_name(&want);
            if let Some(files) = by_name.get(&want).or_else(|| by_name.get(&key_norm)) {
                for f in files {
                    let real = tokio::fs::canonicalize(f)
                        .await
                        .unwrap_or_else(|_| f.clone());
                    if removed_targets.contains(&real) {
                        continue;
                    }
                    match tokio::fs::remove_file(f).await {
                        Ok(_) => {
                            tracing::info!(plugin = %want, path = %f.display(), "Removed plugin file");
                            removed_targets.insert(real);
                            removed_any = true;
                        }
                        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                            tracing::debug!(path = %f.display(), "Plugin file already removed; skipping");
                            removed_targets.insert(real);
                            removed_any = true;
                        }
                        Err(e) => {
                            tracing::warn!(error = %e, path = %f.display(), "Failed to remove plugin file");
                        }
                    }
                }
            } else {
                missing.push(want);
            }
        }

        if !missing.is_empty() {
            tracing::warn!(missing = ?missing, "Requested plugins not found");
        }

        if removed_any {
            for dir in [&plugin_dir, &stable_plugin_dir] {
                if let Ok(mut rd) = tokio::fs::read_dir(dir).await {
                    let mut any_file = false;
                    while let Ok(Some(e)) = rd.next_entry().await {
                        if e.file_type().await.map(|t| t.is_file()).unwrap_or(false) {
                            any_file = true;
                            break;
                        }
                    }
                    if !any_file {
                        if let Err(e) = tokio::fs::remove_dir(dir).await {
                            tracing::debug!(error = %e, dir = %dir.display(), "Failed to remove empty plugin directory");
                        }
                    }
                }
            }
        }

        Ok(())
    }
}

pub fn extract_plugin_name(path: &Path) -> Option<String> {
    let fname = path.file_name()?.to_str()?;
    #[cfg(target_os = "linux")]
    {
        fname
            .strip_prefix("libwasmedgePlugin")
            .and_then(|rest| rest.strip_suffix(".so"))
            .map(|core| core.to_string())
    }
    #[cfg(target_os = "macos")]
    {
        fname
            .strip_prefix("libwasmedgePlugin")
            .and_then(|rest| rest.strip_suffix(".dylib"))
            .map(|core| core.to_string())
    }
    #[cfg(target_os = "windows")]
    {
        fname
            .strip_prefix("wasmedgePlugin")
            .and_then(|rest| rest.strip_suffix(".dll"))
            .map(|core| core.to_string())
    }
}

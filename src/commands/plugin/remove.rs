use std::{
    collections::{BTreeMap, HashSet},
    ffi::OsString,
    io::ErrorKind,
    path::{Path, PathBuf},
};

use clap::Args;

use super::install::select_runtime_version;
use super::utils::extract_plugin_name;
use super::version::PluginVersion;
use crate::{
    cli::{CommandContext, CommandExecutor},
    commands::{
        resolve_normalized_install_path,
        runtime::{open_install_root, open_versions_root},
    },
    error::{Error, Result},
    fs as wfs,
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

        let target_dir = resolve_normalized_install_path(self.path.clone())?;
        let Some(install_root) = open_install_root(&target_dir)? else {
            return Err(Error::VersionNotFound {
                version: self
                    .runtime
                    .clone()
                    .unwrap_or_else(|| "<none installed>".to_string()),
            });
        };
        let Some(versions_root) = open_versions_root(&install_root, &target_dir)? else {
            return Err(Error::VersionNotFound {
                version: self
                    .runtime
                    .clone()
                    .unwrap_or_else(|| "<none installed>".to_string()),
            });
        };
        let (runtime_version, version_root) =
            select_runtime_version(&versions_root, self.runtime.as_deref())?;
        let plugin_dir = target_dir
            .join("versions")
            .join(runtime_version.to_string())
            .join("plugin");
        let plugin_root = match wfs::open_cap_dir_nofollow(&version_root, Path::new("plugin")) {
            Ok(plugin_root) => plugin_root,
            Err(error) if error.kind() == ErrorKind::NotFound => {
                tracing::info!(dir = %plugin_dir.display(), "No plugin directory found to remove from");
                return Ok(());
            }
            Err(source) => {
                return Err(Error::InvalidPath {
                    path: plugin_dir.display().to_string(),
                    reason: format!(
                        "the plugin path must be a real directory opened without following symlinks: {source}"
                    ),
                });
            }
        };

        let mut by_name: BTreeMap<String, Vec<OsString>> = BTreeMap::new();

        for entry in plugin_root.entries()? {
            let entry = entry?;
            let file_type = entry.file_type()?;
            if file_type.is_symlink() || !file_type.is_file() {
                continue;
            }
            let file_name = entry.file_name();
            if let Some(raw_name) = extract_plugin_name(Path::new(&file_name)) {
                let norm = normalize_name(&raw_name);
                by_name.entry(raw_name).or_default().push(file_name.clone());
                by_name.entry(norm).or_default().push(file_name);
            }
        }

        if by_name.is_empty() {
            tracing::info!(
                dir = %plugin_dir.display(),
                "No plugin files found to remove"
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
        let mut removed_targets: HashSet<OsString> = HashSet::new();
        let mut missing: Vec<String> = Vec::new();
        for want in requested {
            let key_norm = normalize_name(&want);
            if let Some(files) = by_name.get(&want).or_else(|| by_name.get(&key_norm)) {
                for file_name in files {
                    if !removed_targets.insert(file_name.clone()) {
                        continue;
                    }
                    let path = plugin_dir.join(file_name);
                    match plugin_root.remove_file(file_name) {
                        Ok(_) => {
                            tracing::info!(plugin = %want, path = %path.display(), "Removed plugin file");
                            removed_any = true;
                        }
                        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                            tracing::debug!(path = %path.display(), "Plugin file already removed; skipping");
                            removed_any = true;
                        }
                        Err(e) => {
                            tracing::warn!(error = %e, path = %path.display(), "Failed to remove plugin file");
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
            drop(plugin_root);
            if let Err(error) = version_root.remove_dir("plugin") {
                if !matches!(
                    error.kind(),
                    ErrorKind::DirectoryNotEmpty | ErrorKind::NotFound
                ) {
                    tracing::debug!(%error, dir = %plugin_dir.display(), "Failed to remove empty plugin directory");
                }
            }
        }

        Ok(())
    }
}

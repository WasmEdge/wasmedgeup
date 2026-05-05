use std::path::{Path, PathBuf};

use clap::{value_parser, Args};
use tokio::fs;
use walkdir::WalkDir;

use crate::api::{plugin_archive_name, plugin_asset_url, WasmEdgeApiClient};
use crate::system::plugins::plugin_platform_key;
use crate::{
    cli::{CommandContext, CommandExecutor},
    commands::default_path,
    error::{Error, Result},
    fs as wfs, system,
};

use super::utils::find_plugin_shared_objects;
use super::version::PluginVersion;

#[derive(Debug, Args)]
pub struct PluginInstallArgs {
    /// Space-separated names and versions of plugins to install, e.g. `plugin1 plugin2@version`
    #[arg(value_parser = value_parser!(PluginVersion))]
    pub plugins: Vec<PluginVersion>,

    /// Optional temporary directory for staging downloads
    #[arg(short, long)]
    pub tmpdir: Option<PathBuf>,

    /// Install plugins into this runtime version (defaults to latest installed)
    #[arg(long, value_name = "RUNTIME_VERSION")]
    pub runtime: Option<String>,

    /// Set the install location for the WasmEdge runtime (defaults to $HOME/.wasmedge)
    #[arg(short, long)]
    pub path: Option<PathBuf>,

    /// Skip checksum retrieval and verification for the downloaded plugin archive.
    ///
    /// This option disables integrity verification against the release-level
    /// SHA256SUM file.
    #[arg(long)]
    pub no_verify: bool,
}

impl PluginInstallArgs {
    fn tmpdir(&self) -> PathBuf {
        self.tmpdir
            .clone()
            .unwrap_or_else(std::env::temp_dir)
            .join("wasmedgeup")
            .join("plugins")
    }
}

impl CommandExecutor for PluginInstallArgs {
    /// Executes the plugin installation process by resolving the target runtime version,
    /// detecting the platform key, downloading the plugin asset, unpacking it, discovering
    /// the plugin shared objects, and copying them into the versioned plugin directory.
    ///
    /// # Steps
    /// 1. Resolve the target runtime version (either a specific version or the latest installed one).
    /// 2. Detect the host system specs and compute the plugin platform key (version-aware for Linux manylinux baseline and Darwin major on macOS).
    /// 3. For each requested plugin, construct the release asset URL and download it to a temporary workspace.
    /// 4. Unpack the archive into the workspace.
    /// 5. Discover plugin artifacts and copy them into `versions/<version>/plugin`.
    /// 6. If no plugin shared objects are found, emit a warning and include a listing of archive contents to aid debugging.
    ///
    /// # Arguments
    /// * `ctx` - The command context containing the HTTP client and progress/settings.
    ///
    /// # Errors
    /// Returns an error if any step fails, such as permissions issues on the version directory,
    /// unsupported platform determination, download failures, extraction errors, or invalid inputs
    #[tracing::instrument(name = "plugin.install", skip_all, fields(plugins = ?self.plugins))]
    async fn execute(self, ctx: CommandContext) -> Result<()> {
        if self.plugins.is_empty() {
            return Err(Error::NoPluginsSpecified);
        }

        let versions_dir = match self.path.clone() {
            Some(p) => p,
            None => default_path()?,
        }
        .join("versions");
        let runtime_version = select_runtime_version(&versions_dir, self.runtime.as_deref())?;
        let version_dir = versions_dir.join(runtime_version.to_string());

        if !version_dir.exists() {
            return Err(Error::VersionNotFound {
                version: runtime_version.to_string(),
            });
        }
        if !wfs::can_write_to_directory(&version_dir) {
            return Err(crate::commands::insufficient_permissions(
                &version_dir,
                "write to target version directory",
                &runtime_version.to_string(),
            ));
        }

        let specs = system::detect();
        let dest_plugin = version_dir.join("plugin");
        fs::create_dir_all(&dest_plugin).await?;

        // Windows ships zip archives; other platforms ship tar.gz. The local
        // boolean is named for the *archive format* (what the call sites
        // actually need) rather than the host OS.
        let is_zip = matches!(specs.os.os_type, crate::target::TargetOS::Windows);

        let tmp_root = self.tmpdir();
        for plugin in &self.plugins {
            // Keep the plugin's own version typed: when the user passes
            // `plugin@version`, the version may differ from `runtime_version`
            // and `plugin_platform_key` is version-aware (manylinux2014 vs
            // manylinux_2_28 boundary at 0.15). Computing os_key against the
            // runtime once would build wrong URLs for `plugin@<older>`
            // installs.
            let (name, pver_semver) = match plugin {
                PluginVersion::Name(n) => (n.as_str(), runtime_version.clone()),
                PluginVersion::NameAndVersion(n, v) => (n.as_str(), v.clone()),
            };
            let pver = pver_semver.to_string();
            let os_key = plugin_platform_key(&specs.os, &pver_semver)?;
            tracing::debug!(%name, %pver, platform_key = %os_key, "Resolved plugin asset platform key");

            let url = plugin_asset_url(name, &pver, &os_key, is_zip)?;
            tracing::debug!(%name, %pver, %url, "Downloading plugin");

            let workspace = tmp_root.join(format!("{name}-{pver}"));
            fs::create_dir_all(&workspace).await?;
            let archive_path = if is_zip {
                workspace.join("plugin.zip")
            } else {
                workspace.join("plugin.tar.gz")
            };

            ctx.client
                .download_to_path(url, &archive_path, ctx.no_progress, "plugin download")
                .await?;

            let mut file = std::fs::OpenOptions::new()
                .read(true)
                .open(&archive_path)
                .map_err(|source| Error::Io {
                    action: "open archive".to_string(),
                    path: archive_path.display().to_string(),
                    source,
                })?;

            if self.no_verify {
                tracing::warn!(plugin = %name, "Skipping plugin checksum verification due to --no-verify flag");
            } else {
                let archive_name = plugin_archive_name(name, &pver, &os_key, is_zip);
                let expected = ctx
                    .client
                    .get_archive_checksum(&pver, &archive_name)
                    .await
                    .inspect_err(
                        |e| tracing::error!(error = %e, "Failed to get plugin checksum"),
                    )?;
                tracing::debug!(plugin = %name, checksum = %expected, "Got plugin checksum");
                WasmEdgeApiClient::verify_file_checksum(&mut file, &expected).await?;
                tracing::debug!(plugin = %name, "Plugin checksum verified");
            }

            wfs::extract_archive(&mut file, &workspace).await?;

            let paths = find_plugin_shared_objects(&workspace);
            let found_any = if !paths.is_empty() {
                for src in paths {
                    let file_name = src.file_name().unwrap_or_default();
                    let dest = dest_plugin.join(file_name);
                    if let Some(parent) = dest.parent() {
                        if let Err(e) = fs::create_dir_all(parent).await {
                            tracing::warn!(error = %e, path = %parent.display(), "Failed to create parent directory for plugin");
                        }
                    }
                    if let Err(e) = fs::copy(&src, &dest).await {
                        tracing::warn!(error = %e, from = %src.display(), to = %dest.display(), "Failed to copy plugin shared object");
                    } else {
                        tracing::debug!(from = %src.display(), to = %dest.display(), "Copied plugin shared object");
                    }
                }
                true
            } else {
                false
            };

            if !found_any {
                let mut entries: Vec<String> = Vec::new();
                for e in WalkDir::new(&workspace).into_iter().filter_map(|e| e.ok()) {
                    let p = e.path();
                    if p.is_file() {
                        let rel = p.strip_prefix(&workspace).unwrap_or(p);
                        entries.push(rel.display().to_string());
                    }
                }
                tracing::warn!(
                    root = %workspace.display(),
                    entries = ?entries,
                    "No plugin shared object found in archive; nothing was installed"
                );
            }

            if let Err(e) = fs::remove_dir_all(&workspace).await {
                tracing::debug!(error = %e, path = %workspace.display(), "Failed to cleanup workspace");
            }

            tracing::info!(plugin = %name, version = %pver, "Installed plugin successfully");
        }

        Ok(())
    }
}

pub(super) fn select_runtime_version(
    versions_dir: &Path,
    requested: Option<&str>,
) -> Result<semver::Version> {
    if let Some(ver) = requested {
        return semver::Version::parse(ver).map_err(|source| Error::SemVer { source });
    }
    match crate::api::latest_installed_version(versions_dir)? {
        Some(v) => Ok(v),
        None => Err(Error::VersionNotFound {
            version: "<none installed>".to_string(),
        }),
    }
}

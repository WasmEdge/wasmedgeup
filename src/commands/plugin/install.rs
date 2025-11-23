use std::path::{Path, PathBuf};

use clap::{value_parser, Args};
use tokio::fs;
use walkdir::WalkDir;

use crate::system::plugins::plugin_platform_key;
use crate::{
    cli::{CommandContext, CommandExecutor},
    commands::default_path,
    error::{Error, Result},
    fs as wfs, system,
};

use super::version::PluginVersion;

const GH_RELEASE_DOWNLOAD_BASE: &str = "https://github.com/WasmEdge/WasmEdge/releases/download";

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
        if !wfs::can_write_to_directory(&version_dir) {
            return Err(crate::commands::insufficient_permissions(
                &version_dir,
                "write to target version directory",
                &runtime_version.to_string(),
            ));
        }

        let specs = system::detect();
        let os_key = plugin_platform_key(&specs.os, &runtime_version)?;
        tracing::debug!(platform_key = %os_key, "Resolved plugin platform key for plugins");

        let dest_plugin = version_dir.join("plugin");
        fs::create_dir_all(&dest_plugin).await?;

        let tmp_root = self.tmpdir();
        for plugin in &self.plugins {
            let (name, pver) = match plugin {
                PluginVersion::Name(n) => (n.as_str(), runtime_version.to_string()),
                PluginVersion::NameAndVersion(n, v) => (n.as_str(), v.to_string()),
            };

            let is_windows = matches!(specs.os.os_type, crate::target::TargetOS::Windows);
            let ext = if is_windows { "zip" } else { "tar.gz" };
            let url = format!(
                "{base}/{ver}/WasmEdge-plugin-{name}-{ver}-{os_key}.{ext}",
                base = GH_RELEASE_DOWNLOAD_BASE,
                name = name,
                ver = pver,
                os_key = os_key,
                ext = ext,
            );
            tracing::debug!(%name, %pver, %url, "Downloading plugin");

            let workspace = tmp_root.join(format!("{name}-{pver}"));
            fs::create_dir_all(&workspace).await?;
            let archive_path = if is_windows {
                workspace.join("plugin.zip")
            } else {
                workspace.join("plugin.tar.gz")
            };

            download_with_progress(&ctx, &url, &archive_path).await?;

            let mut file = std::fs::OpenOptions::new()
                .read(true)
                .open(&archive_path)
                .map_err(|source| Error::Io {
                    action: "open archive".to_string(),
                    path: archive_path.display().to_string(),
                    source,
                })?;
            wfs::extract_archive(&mut file, &workspace).await?;

            let found_any = match find_plugin_shared_objects(&workspace) {
                Ok(paths) if !paths.is_empty() => {
                    for src in paths {
                        let file_name = src.file_name().unwrap_or_default();
                        let dest = dest_plugin.join(file_name);
                        if let Some(parent) = dest.parent() {
                            let _ = fs::create_dir_all(parent).await;
                        }
                        if let Err(e) = fs::copy(&src, &dest).await {
                            tracing::warn!(error = %e, from = %src.display(), to = %dest.display(), "Failed to copy plugin shared object");
                        } else {
                            tracing::debug!(from = %src.display(), to = %dest.display(), "Copied plugin shared object");
                        }
                    }
                    true
                }
                _ => false,
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

async fn download_with_progress(ctx: &CommandContext, url: &str, to: &Path) -> Result<()> {
    use tokio::io::AsyncWriteExt as _;

    let client = reqwest::ClientBuilder::new()
        .connect_timeout(std::time::Duration::from_secs(ctx.client.connect_timeout))
        .timeout(std::time::Duration::from_secs(ctx.client.request_timeout))
        .user_agent(format!(
            "wasmedgeup/{} (+https://github.com/WasmEdge/wasmedgeup)",
            env!("CARGO_PKG_VERSION")
        ))
        .build()
        .expect("Failed to build reqwest client");

    let resp = client
        .get(url)
        .send()
        .await
        .map_err(|source| Error::Request {
            source,
            resource: "plugin download",
        })?;

    let resp = resp.error_for_status().map_err(|source| Error::Request {
        source,
        resource: "plugin download",
    })?;

    let bytes = resp.bytes().await.map_err(|source| Error::Request {
        source,
        resource: "plugin download body",
    })?;

    let mut file = tokio::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(to)
        .await?;
    file.write_all(&bytes).await?;
    Ok(())
}

/// Recursively scans an extracted plugin archive to find plugin shared objects.
///
/// Patterns per platform:
/// - Linux: files matching `libwasmedgePlugin*.so`
/// - macOS: files matching `libwasmedgePlugin*.dylib`
/// - Windows: files matching `wasmedgePlugin*.dll`
///
/// Notes:
/// - Ignores the `__MACOSX` metadata directory
/// - Returns a list of absolute paths to matching files.
fn find_plugin_shared_objects(root: &Path) -> Result<Vec<PathBuf>> {
    let mut results = Vec::new();
    for entry in WalkDir::new(root).into_iter().filter_map(|e| e.ok()) {
        let path = entry.path();
        if let Some(name) = path.file_name().and_then(|s| s.to_str()) {
            if name == "__MACOSX" {
                continue;
            }
        }
        if !entry.file_type().is_file() {
            continue;
        }
        let Some(fname) = path.file_name().and_then(|s| s.to_str()) else {
            continue;
        };
        #[cfg(target_os = "linux")]
        {
            if fname.starts_with("libwasmedgePlugin") && fname.ends_with(".so") {
                results.push(path.to_path_buf());
            }
        }
        #[cfg(target_os = "macos")]
        {
            if fname.starts_with("libwasmedgePlugin") && fname.ends_with(".dylib") {
                results.push(path.to_path_buf());
            }
        }
        #[cfg(target_os = "windows")]
        {
            if fname.starts_with("wasmedgePlugin") && fname.ends_with(".dll") {
                results.push(path.to_path_buf());
            }
        }
    }
    Ok(results)
}

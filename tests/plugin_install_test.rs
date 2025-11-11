use std::path::{Path, PathBuf};

use tempfile::{tempdir, TempDir};
use wasmedgeup::{
    api::WasmEdgeApiClient,
    cli::{CommandContext, CommandExecutor},
    commands::install::InstallArgs,
    commands::plugin::{install::PluginInstallArgs, version::PluginVersion},
    system,
};

mod test_utils;
use test_utils::setup_test_environment;

async fn execute_runtime_install(version: String, install_dir: &Path, tmpdir: &TempDir) {
    let args = InstallArgs {
        version,
        path: Some(install_dir.to_path_buf()),
        tmpdir: Some(tmpdir.path().to_path_buf()),
        os: None,
        arch: None,
    };

    let client = WasmEdgeApiClient::default();
    let ctx = CommandContext {
        client,
        no_progress: false,
    };

    args.execute(ctx).await.expect("runtime install failed");
}

async fn execute_plugin_install(
    plugins: Vec<PluginVersion>,
    install_dir: PathBuf,
    tmpdir: TempDir,
    runtime: Option<String>,
) {
    let args = PluginInstallArgs {
        plugins,
        tmpdir: Some(tmpdir.path().to_path_buf()),
        runtime,
        path: Some(install_dir.clone()),
    };

    let client = WasmEdgeApiClient::default();
    let ctx = CommandContext {
        client,
        no_progress: false,
    };

    args.execute(ctx).await.expect("plugin install failed");

    assert!(install_dir.exists());
    let plugin_dir = install_dir.join("plugin");
    assert!(
        plugin_dir.exists(),
        "plugin directory not found: {}",
        plugin_dir.display()
    );
}

use wasmedgeup::system::plugins::plugin_platform_key;
use wasmedgeup::target::TargetOS;

#[tokio::test]
async fn test_plugin_install_latest_runtime() {
    let tmpdir = tempdir().unwrap();
    let install_dir = tmpdir.path().join("install_target");

    let (_tempdir, _test_home) = setup_test_environment();

    let version = "latest".to_string();
    execute_runtime_install(version.clone(), &install_dir, &tmpdir).await;

    let client = WasmEdgeApiClient::default();
    let resolved_version = client
        .resolve_version(&version)
        .expect("resolve latest failed");

    let specs = system::detect();
    let key =
        plugin_platform_key(&specs.os, &resolved_version).expect("compute plugin platform key");
    let candidates = ["wasi_crypto", "wasi_nn", "wasi_logging"];
    let http = reqwest::Client::new();
    let mut chosen: Option<String> = None;
    for name in candidates {
        let ext = if matches!(specs.os.os_type, TargetOS::Windows) {
            "zip"
        } else {
            "tar.gz"
        };
        let url = format!(
            "https://github.com/WasmEdge/WasmEdge/releases/download/{ver}/WasmEdge-plugin-{name}-{ver}-{key}.{ext}",
            ver = resolved_version,
            name = name,
            key = key,
            ext = ext,
        );
        if let Ok(resp) = http.head(&url).send().await {
            if resp.status().is_success() {
                chosen = Some(name.to_string());
                break;
            }
        }
    }
    let Some(plugin_name) = chosen else {
        eprintln!(
            "No plugin asset available for version {} and key {}; skipping",
            resolved_version, key
        );
        return;
    };

    let plugins = vec![PluginVersion::Name(plugin_name.clone())];
    execute_plugin_install(
        plugins,
        install_dir.clone(),
        tmpdir,
        Some(resolved_version.to_string()),
    )
    .await;

    let plugin_dir = install_dir
        .join("versions")
        .join(resolved_version.to_string())
        .join("plugin");
    let mut found = false;
    if let Ok(rd) = std::fs::read_dir(&plugin_dir) {
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().to_string();
            #[cfg(target_os = "linux")]
            if name.starts_with("libwasmedgePlugin") && name.ends_with(".so") {
                found = true;
                break;
            }
            #[cfg(target_os = "macos")]
            if name.starts_with("libwasmedgePlugin") && name.ends_with(".dylib") {
                found = true;
                break;
            }
            #[cfg(target_os = "windows")]
            if name.starts_with("wasmedgePlugin") && name.ends_with(".dll") {
                found = true;
                break;
            }
        }
    }
    if !found {
        eprintln!(
            "No plugin shared object found in {}; skipping",
            plugin_dir.display()
        );
        return;
    }
}

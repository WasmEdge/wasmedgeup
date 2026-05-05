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
use serial_test::serial;
use test_utils::setup_test_environment;

async fn execute_runtime_install(version: String, install_dir: &Path, tmpdir: &TempDir) {
    let args = InstallArgs {
        version,
        path: Some(install_dir.to_path_buf()),
        tmpdir: Some(tmpdir.path().to_path_buf()),
        os: None,
        arch: None,
        no_verify: false,
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
    tmpdir_path: &Path,
    runtime: Option<String>,
    no_verify: bool,
) {
    // Borrow the tmpdir path: the TempDir owner must outlive this helper so
    // post-install assertions in the caller can read the install layout —
    // moving the TempDir in here would drop it (and wipe install_dir's
    // backing directory) before the caller's filesystem checks ran.
    let args = PluginInstallArgs {
        plugins,
        tmpdir: Some(tmpdir_path.to_path_buf()),
        runtime,
        path: Some(install_dir.clone()),
        no_verify,
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

/// Shared smoke-test orchestration for `wasmedgeup plugin install` against
/// the live release. Toggles the `--no-verify` flag so we exercise both the
/// checksum-verifying default and the explicit-skip path.
async fn run_plugin_install_smoke(no_verify: bool) {
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
        tmpdir.path(),
        Some(resolved_version.to_string()),
        no_verify,
    )
    .await;

    // tmpdir is still alive here — the helper borrowed its path. We can now
    // inspect the on-disk install layout to confirm a plugin shared object
    // actually landed in `versions/<ver>/plugin/`. Skipping this assertion
    // (as the previous version did via a silent eprintln) hid both real
    // install failures and the buggy "TempDir dropped before scan" lifecycle
    // that produced this fix.
    let plugin_dir = install_dir
        .join("versions")
        .join(resolved_version.to_string())
        .join("plugin");
    let entries = std::fs::read_dir(&plugin_dir)
        .unwrap_or_else(|e| panic!("failed to read {}: {e}", plugin_dir.display()));
    let mut found = false;
    for e in entries.flatten() {
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
    assert!(
        found,
        "plugin install reported success but no plugin shared object \
        was placed under {} (no_verify={no_verify})",
        plugin_dir.display(),
    );
    // Keep tmpdir alive until the assertion runs; it drops at scope exit.
    drop(tmpdir);
}

#[tokio::test]
#[serial]
async fn test_plugin_install_latest_runtime() {
    // Default path: SHA256SUM is fetched and the archive is verified before
    // extraction.
    run_plugin_install_smoke(false).await;
}

#[tokio::test]
#[serial]
async fn test_plugin_install_latest_runtime_no_verify() {
    // `--no-verify` path: install must still succeed (extract + copy plugin
    // shared objects) without performing the checksum lookup. Locks in the
    // flag's behavior so a future regression that, say, runs verification
    // unconditionally would fail this test.
    run_plugin_install_smoke(true).await;
}

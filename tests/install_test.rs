use std::path::PathBuf;

use tempfile::{tempdir, TempDir};
use wasmedgeup::{
    api::{releases, ReleasesFilter, WasmEdgeApiClient},
    cli::{CommandContext, CommandExecutor},
    commands::install::InstallArgs,
};

const WASM_EDGE_GIT_URL: &str = "https://github.com/WasmEdge/WasmEdge.git";

async fn execute_install_test(version: String, install_dir: PathBuf, tmpdir: TempDir) {
    let args = InstallArgs {
        version,
        path: Some(install_dir.clone()),
        tmpdir: Some(tmpdir.path().to_path_buf()),
        os: None,
        arch: None,
    };

    let client = WasmEdgeApiClient::default();
    let ctx = CommandContext {
        client,
        no_progress: false,
    };

    args.execute(ctx).await.expect("install failed");

    assert!(install_dir.exists());
    assert!(install_dir.read_dir().unwrap().next().is_some());

    #[cfg(unix)]
    assert!(install_dir.join("bin/wasmedge").exists());

    #[cfg(windows)]
    assert!(install_dir.join("bin/wasmedge.exe").exists());
}

#[tokio::test]
async fn test_install_latest_version() {
    let tmpdir = tempdir().unwrap();
    let install_dir = tmpdir.path().join("install_target");

    let all_releases = releases::get_all(WASM_EDGE_GIT_URL, ReleasesFilter::Stable).unwrap();
    assert!(!all_releases.is_empty());

    execute_install_test(all_releases[0].to_string(), install_dir, tmpdir).await;
}

#[tokio::test]
async fn test_install_prerelease_version() {
    let tmpdir = tempdir().unwrap();
    let install_dir = tmpdir.path().join("install_target");

    let all_releases = releases::get_all(WASM_EDGE_GIT_URL, ReleasesFilter::All).unwrap();
    assert!(!all_releases.is_empty());

    execute_install_test(all_releases[0].to_string(), install_dir, tmpdir).await;
}

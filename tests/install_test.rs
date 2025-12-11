use std::path::PathBuf;

use semver::Version as SemVersion;
use tempfile::{tempdir, TempDir};
use wasmedgeup::system;
use wasmedgeup::{
    api::{releases, Asset, ReleasesFilter, WasmEdgeApiClient},
    cli::{CommandContext, CommandExecutor},
    commands::install::InstallArgs,
};

mod test_utils;
use test_utils::setup_test_environment;

const WASM_EDGE_GIT_URL: &str = "https://github.com/WasmEdge/WasmEdge.git";

/// From a list of versions (tags), return the first prerelease that has a
/// published asset for the current platform (checked via a HEAD request).
async fn first_available_prerelease(versions: Vec<SemVersion>) -> Option<SemVersion> {
    let specs = system::detect();
    let http = reqwest::Client::new();

    for v in versions.into_iter().filter(|v| !v.pre.is_empty()) {
        let asset = Asset::new(&v, &specs.os.os_type, &specs.os.arch);
        let url = match asset.url() {
            Ok(u) => u,
            Err(_) => continue,
        };
        if let Ok(resp) = http.head(url.clone()).send().await {
            if resp.status().is_success() {
                return Some(v);
            }
        }
    }
    None
}

async fn execute_install_test(
    version: String,
    install_dir: PathBuf,
    tmpdir: TempDir,
    no_verify: bool,
) {
    let args = InstallArgs {
        version,
        path: Some(install_dir.clone()),
        tmpdir: Some(tmpdir.path().to_path_buf()),
        os: None,
        arch: None,
        no_verify,
    };

    let client = WasmEdgeApiClient::default();
    let ctx = CommandContext {
        client,
        no_progress: false,
    };

    args.execute(ctx).await.expect("install failed");

    assert!(install_dir.exists());
    assert!(install_dir.read_dir().unwrap().next().is_some());

    let wasmedge_binary = if cfg!(windows) {
        install_dir.join("bin").join("wasmedge.exe")
    } else {
        install_dir.join("bin").join("wasmedge")
    };

    assert!(
        wasmedge_binary.exists(),
        "WasmEdge binary not found at: {}",
        wasmedge_binary.display()
    );
}

#[tokio::test]
async fn test_install_latest_version() {
    let tmpdir = tempdir().unwrap();
    let install_dir = tmpdir.path().join("install_target");

    let all_releases = releases::get_all(WASM_EDGE_GIT_URL, ReleasesFilter::Stable).unwrap();
    assert!(!all_releases.is_empty());

    let (_tempdir, _test_home) = setup_test_environment();
    #[cfg(windows)]
    {
        // Give Windows a moment to release any file handles
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    execute_install_test(all_releases[0].to_string(), install_dir, tmpdir, false).await;
}

#[tokio::test]
async fn test_install_prerelease_version() {
    let tmpdir = tempdir().unwrap();
    let install_dir = tmpdir.path().join("install_target");

    let all_releases = releases::get_all(WASM_EDGE_GIT_URL, ReleasesFilter::All).unwrap();
    assert!(!all_releases.is_empty());

    let (_tempdir, _test_home) = setup_test_environment();
    #[cfg(windows)]
    {
        // Give Windows a moment to release any file handles
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    let Some(prerelease) = first_available_prerelease(all_releases).await else {
        eprintln!("No prerelease with assets found; skipping");
        return;
    };
    assert!(
        !prerelease.pre.is_empty(),
        "Selected version is not a prerelease"
    );
    execute_install_test(prerelease.to_string(), install_dir, tmpdir, true).await;
}

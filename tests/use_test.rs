use std::path::Path;

use serial_test::serial;
use wasmedgeup::{
    api::{releases, ReleasesFilter, WasmEdgeApiClient},
    cli::{CommandContext, CommandExecutor},
    commands::use_cmd::UseArgs,
};

mod test_utils;

const WASM_EDGE_GIT_URL: &str = "https://github.com/WasmEdge/WasmEdge.git";

#[tokio::test]
#[serial]
async fn test_use_version() {
    let (_tempdir, test_home) = test_utils::setup_test_environment();

    let versions = ["0.14.1", "0.15.0"];
    for version in &versions {
        let version_dir = test_home.join("versions").join(version);
        let bin_dir = version_dir.join("bin");
        let lib_dir = version_dir.join("lib");
        let include_dir = version_dir.join("include");

        tokio::fs::create_dir_all(&bin_dir).await.unwrap();
        tokio::fs::create_dir_all(&lib_dir).await.unwrap();
        tokio::fs::create_dir_all(&include_dir).await.unwrap();

        tokio::fs::write(bin_dir.join("wasmedge"), format!("mock wasmedge {version}"))
            .await
            .unwrap();
    }
    let args = UseArgs {
        version: "0.14.1".to_string(),
        path: Some(test_home.clone()),
    };
    let ctx = CommandContext::default();
    args.execute(ctx).await.unwrap();

    verify_symlinks(&test_home, "0.14.1").await;

    let args = UseArgs {
        version: "0.15.0".to_string(),
        path: Some(test_home.clone()),
    };
    let ctx = CommandContext::default();
    args.execute(ctx).await.unwrap();

    verify_symlinks(&test_home, "0.15.0").await;
}

#[tokio::test]
#[serial]
async fn test_use_latest_version() {
    let (_tempdir, test_home) = test_utils::setup_test_environment();

    let all_releases = releases::get_all(WASM_EDGE_GIT_URL, ReleasesFilter::Stable).unwrap();
    assert!(!all_releases.is_empty());
    let latest_version = &all_releases[0].to_string();
    let version_dir = test_home.join("versions").join(latest_version);
    let bin_dir = version_dir.join("bin");
    let lib_dir = version_dir.join("lib");
    let include_dir = version_dir.join("include");

    tokio::fs::create_dir_all(&bin_dir).await.unwrap();
    tokio::fs::create_dir_all(&lib_dir).await.unwrap();
    tokio::fs::create_dir_all(&include_dir).await.unwrap();

    tokio::fs::write(
        bin_dir.join("wasmedge"),
        format!("mock wasmedge {latest_version}"),
    )
    .await
    .unwrap();

    let args = UseArgs {
        version: "latest".to_string(),
        path: Some(test_home.clone()),
    };
    let ctx = CommandContext {
        client: WasmEdgeApiClient::default(),
        no_progress: true,
    };
    args.execute(ctx).await.unwrap();

    verify_symlinks(&test_home, latest_version).await;
}

#[tokio::test]
#[serial]
async fn test_use_nonexistent_version() {
    let (_tempdir, test_home) = test_utils::setup_test_environment();

    let args = UseArgs {
        version: "0.99.99".to_string(),
        path: Some(test_home),
    };
    let ctx = CommandContext::default();
    let result = args.execute(ctx).await;

    assert!(result.is_err(), "Using non-existent version should fail");
    assert!(matches!(
        result.unwrap_err(),
        wasmedgeup::error::Error::VersionNotFound { version: _ }
    ));
}

#[tokio::test]
#[serial]
async fn test_use_latest_with_no_installs_returns_version_not_found() {
    // No versions installed → `use latest` must surface the empty-install
    // case as VersionNotFound rather than crashing or hitting the network.
    let (_tempdir, test_home) = test_utils::setup_test_environment();

    let args = UseArgs {
        version: "latest".to_string(),
        path: Some(test_home),
    };
    let ctx = CommandContext::default();
    let result = args.execute(ctx).await;

    match result {
        Err(wasmedgeup::error::Error::VersionNotFound { version }) => {
            assert_eq!(
                version, "latest",
                "no-installs case should report \"latest\" as the missing version, not an internal sentinel"
            );
        }
        other => panic!("expected VersionNotFound {{ version: \"latest\" }}, got {other:?}"),
    }
}

#[tokio::test]
#[serial]
async fn test_use_latest_picks_highest_local_version_not_remote() {
    // Locks in the Phase-7 contract: `use latest` resolves against the
    // **local** versions/ tree only, never the network.
    //
    // We install two old versions locally (both predate any plausible
    // upstream "latest stable") and assert `use latest` switches to 0.14.1.
    // Under the previous implementation, `latest` would resolve via git2 to
    // some newer upstream tag (0.15.x+), then fail VersionNotFound because
    // the resolved version isn't installed locally — so this test would
    // fail on that path. Locking in success here proves we never hit the
    // network for "latest".
    let (_tempdir, test_home) = test_utils::setup_test_environment();

    for version in ["0.13.5", "0.14.1"] {
        let version_dir = test_home.join("versions").join(version);
        for sub in ["bin", "lib", "include"] {
            tokio::fs::create_dir_all(version_dir.join(sub))
                .await
                .unwrap();
        }
        tokio::fs::write(
            version_dir.join("bin").join("wasmedge"),
            format!("mock wasmedge {version}"),
        )
        .await
        .unwrap();
    }

    let args = UseArgs {
        version: "latest".to_string(),
        path: Some(test_home.clone()),
    };
    let ctx = CommandContext::default();
    args.execute(ctx)
        .await
        .expect("`use latest` should succeed when local versions are installed");

    // Highest local semver wins; 0.13.5 must lose to 0.14.1.
    verify_symlinks(&test_home, "0.14.1").await;
}

async fn verify_symlinks(base_dir: &Path, expected_version: &str) {
    for dir in ["bin", "lib", "include"] {
        let symlink = base_dir.join(dir);
        assert!(symlink.exists(), "Symlink {dir} should exist");

        let target = tokio::fs::read_link(&symlink).await.unwrap();

        #[cfg(windows)]
        {
            let expected_rel = std::path::Path::new("versions")
                .join(expected_version)
                .join(dir);
            assert!(
                target.ends_with(&expected_rel),
                "Symlink {dir} should point to version {expected_version}. target='{}' expected_suffix='{}'",
                target.display(),
                expected_rel.display()
            );
        }

        #[cfg(unix)]
        {
            let expected = format!("versions/{expected_version}/{dir}");
            assert_eq!(
                target.to_string_lossy(),
                expected,
                "Symlink {dir} should point to version {expected_version}"
            );
        }
    }
}

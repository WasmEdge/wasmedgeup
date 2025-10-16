use std::path::Path;

use wasmedgeup::{
    api::{latest_installed_version, WasmEdgeApiClient},
    cli::{CommandContext, CommandExecutor},
    commands::remove::RemoveArgs,
    error::Error,
};

mod test_utils;

#[tokio::test]
async fn test_remove_single_version() {
    let (_tempdir, test_home) = test_utils::setup_test_environment();

    let version = "0.14.1";
    let version_dir = test_home.join("versions").join(version);
    setup_mock_version(&version_dir, version).await;

    let remove_args = RemoveArgs {
        version: version.to_string(),
        all: false,
        path: Some(test_home.clone()),
    };
    let ctx = CommandContext {
        client: WasmEdgeApiClient::default(),
        no_progress: true,
    };
    remove_args.execute(ctx).await.unwrap();

    assert!(!version_dir.exists(), "Version directory should be removed");
}

#[tokio::test]
async fn test_remove_multiple_versions() {
    let (_tempdir, test_home) = test_utils::setup_test_environment();

    let ordered_versions = ["0.20.0", "0.14.1", "0.14.1-rc.1", "0.9.0"];
    for version in &ordered_versions {
        let version_dir = test_home.join("versions").join(version);
        setup_mock_version(&version_dir, version).await;
    }

    let bin_link = test_home.join("bin");

    for (idx, version) in ordered_versions.iter().enumerate() {
        let remove_args = RemoveArgs {
            version: (*version).to_string(),
            all: false,
            path: Some(test_home.clone()),
        };
        let ctx = CommandContext {
            client: WasmEdgeApiClient::default(),
            no_progress: true,
        };
        remove_args.execute(ctx).await.unwrap();

        let is_last = idx + 1 == ordered_versions.len();
        if !is_last {
            let versions_dir = test_home.join("versions");
            let latest = latest_installed_version(&versions_dir)
                .expect("latest_installed_version should succeed")
                .expect("there should be at least one remaining version");
            let expected_next = Path::new("versions").join(latest.to_string()).join("bin");

            let new_target = std::fs::read_link(&bin_link).expect("bin symlink should exist");
            let normalized = if new_target.is_absolute() {
                new_target
                    .strip_prefix(&test_home)
                    .map(|p| p.to_path_buf())
                    .unwrap_or(new_target)
            } else {
                new_target
            };

            assert_eq!(
                normalized, expected_next,
                "bin symlink should switch to next latest"
            );
        } else {
            assert!(
                !test_home.exists(),
                "Install root should be removed after last version is deleted"
            );
        }
    }
}

#[tokio::test]
async fn test_remove_all_versions() {
    let (_tempdir, test_home) = test_utils::setup_test_environment();

    let versions = ["0.14.1", "0.15.0"];
    for version in &versions {
        let version_dir = test_home.join("versions").join(version);
        setup_mock_version(&version_dir, version).await;
    }

    let remove_args = RemoveArgs {
        version: String::new(),
        all: true,
        path: Some(test_home.clone()),
    };
    let ctx = CommandContext {
        client: WasmEdgeApiClient::default(),
        no_progress: true,
    };
    remove_args.execute(ctx).await.unwrap();

    let versions_dir = test_home.join("versions");
    assert!(
        !versions_dir.exists(),
        "Versions directory should be removed"
    );
}

#[tokio::test]
async fn test_remove_nonexistent_version() {
    let (_tempdir, test_home) = test_utils::setup_test_environment();

    let remove_args = RemoveArgs {
        version: "0.99.99".to_string(),
        all: false,
        path: Some(test_home),
    };
    let ctx = CommandContext {
        client: WasmEdgeApiClient::default(),
        no_progress: true,
    };
    let result = remove_args.execute(ctx).await;
    assert!(
        matches!(result, Err(Error::VersionNotFound { .. })),
        "expected VersionNotFound error, got: {result:?}"
    );
}

async fn setup_mock_version(version_dir: &Path, version: &str) {
    let bin_dir = version_dir.join("bin");
    let lib_dir = version_dir.join("lib");
    let include_dir = version_dir.join("include");

    tokio::fs::create_dir_all(&bin_dir).await.unwrap();
    tokio::fs::create_dir_all(&lib_dir).await.unwrap();
    tokio::fs::create_dir_all(&include_dir).await.unwrap();

    tokio::fs::write(bin_dir.join("wasmedge"), format!("mock wasmedge {version}"))
        .await
        .unwrap();

    let install_root = version_dir
        .parent()
        .and_then(|p| p.parent())
        .expect("version_dir should be <root>/versions/<ver>");
    let bin_link = install_root.join("bin");

    if !bin_link.exists() {
        let target_rel = Path::new("versions").join(version).join("bin");

        #[cfg(unix)]
        {
            use std::os::unix::fs as unix_fs;
            unix_fs::symlink(&target_rel, &bin_link)
                .expect("failed to create unix symlink for bin");
        }

        #[cfg(windows)]
        {
            use std::os::windows::fs as win_fs;
            win_fs::symlink_dir(&target_rel, &bin_link)
                .expect("failed to create windows symlink for bin");
        }
    }
}

use std::path::Path;

use serial_test::serial;
use wasmedgeup::{
    api::{latest_installed_version, WasmEdgeApiClient},
    cli::{CommandContext, CommandExecutor},
    commands::remove::RemoveArgs,
    error::Error,
};

mod test_utils;

#[tokio::test]
#[serial]
async fn test_remove_single_version() {
    let (_tempdir, home_dir) = test_utils::setup_test_environment();
    let test_home = home_dir.join(".wasmedge");

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
#[serial]
async fn test_remove_multiple_versions() {
    let (_tempdir, home_dir) = test_utils::setup_test_environment();
    let test_home = home_dir.join(".wasmedge");

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
#[serial]
async fn test_remove_last_version_ignores_unmanaged_versions_directories() {
    let (_tempdir, home_dir) = test_utils::setup_test_environment();
    let test_home = home_dir.join(".wasmedge");
    let versions_dir = test_home.join("versions");

    let version_dir = versions_dir.join("0.14.1");
    setup_mock_version(&version_dir, "0.14.1").await;
    let foreign_file = versions_dir.join("backups").join("do-not-delete");
    tokio::fs::create_dir_all(foreign_file.parent().unwrap())
        .await
        .unwrap();
    tokio::fs::write(&foreign_file, "unrelated data")
        .await
        .unwrap();
    let foreign_semver_file = versions_dir.join("9.9.9").join("do-not-delete");
    tokio::fs::create_dir_all(foreign_semver_file.parent().unwrap())
        .await
        .unwrap();
    tokio::fs::write(&foreign_semver_file, "unrelated semantic-version data")
        .await
        .unwrap();
    #[cfg(unix)]
    let foreign_semver_link = {
        use std::os::unix::fs::symlink;

        let path = test_home.join("include");
        symlink(Path::new("versions/9.9.9/include"), &path).unwrap();
        path
    };

    let remove_args = RemoveArgs {
        version: "0.14.1".to_string(),
        all: false,
        path: Some(test_home.clone()),
    };
    remove_args
        .execute(CommandContext {
            client: WasmEdgeApiClient::default(),
            no_progress: true,
        })
        .await
        .unwrap();

    assert!(foreign_file.exists(), "unmanaged versions data must remain");
    assert!(
        foreign_semver_file.exists(),
        "an unowned semantic-version directory must not count as an installed runtime"
    );
    #[cfg(unix)]
    assert_eq!(
        std::fs::read_link(foreign_semver_link).unwrap(),
        Path::new("versions/9.9.9/include"),
        "a root symlink into an unowned semver directory must remain"
    );
    assert!(
        std::fs::symlink_metadata(test_home.join("bin")).is_err(),
        "managed root links must be removed when no installed versions remain"
    );
}

#[cfg(unix)]
#[tokio::test]
#[serial]
async fn test_remove_current_preserves_unmanaged_links_when_switching() {
    use std::os::unix::fs::symlink;

    let (_tempdir, home_dir) = test_utils::setup_test_environment();
    let test_home = home_dir.join(".wasmedge");
    setup_mock_version(&test_home.join("versions").join("0.14.1"), "0.14.1").await;
    setup_mock_version(&test_home.join("versions").join("0.15.0"), "0.15.0").await;

    let foreign_target = tempfile::tempdir().unwrap();
    let foreign_link = test_home.join("lib");
    symlink(foreign_target.path(), &foreign_link).unwrap();

    let remove_args = RemoveArgs {
        version: "0.14.1".to_string(),
        all: false,
        path: Some(test_home.clone()),
    };
    remove_args
        .execute(CommandContext {
            client: WasmEdgeApiClient::default(),
            no_progress: true,
        })
        .await
        .unwrap();

    assert_eq!(
        std::fs::read_link(&foreign_link).unwrap(),
        foreign_target.path(),
        "switching versions must preserve unrelated root symlinks"
    );
    for name in ["bin", "include"] {
        assert_eq!(
            std::fs::read_link(test_home.join(name)).unwrap(),
            Path::new("versions").join("0.15.0").join(name),
            "managed or missing links must point to the remaining version"
        );
    }
}

#[cfg(unix)]
#[tokio::test]
#[serial]
async fn test_remove_retargets_each_link_that_points_to_removed_version() {
    use std::os::unix::fs::symlink;

    let (_tempdir, home_dir) = test_utils::setup_test_environment();
    let test_home = home_dir.join(".wasmedge");
    setup_mock_version(&test_home.join("versions").join("0.15.0"), "0.15.0").await;
    setup_mock_version(&test_home.join("versions").join("0.14.1"), "0.14.1").await;

    let include_link = test_home.join("include");
    symlink("versions/0.14.1/include", &include_link).unwrap();

    RemoveArgs {
        version: "0.14.1".to_string(),
        all: false,
        path: Some(test_home.clone()),
    }
    .execute(CommandContext {
        client: WasmEdgeApiClient::default(),
        no_progress: true,
    })
    .await
    .unwrap();

    assert_eq!(
        std::fs::read_link(test_home.join("bin")).unwrap(),
        Path::new("versions/0.15.0/bin"),
        "a link to another managed version must remain unchanged"
    );
    assert_eq!(
        std::fs::read_link(include_link).unwrap(),
        Path::new("versions/0.15.0/include"),
        "each managed link to the removed version must be retargeted"
    );
    for name in ["lib", "plugin"] {
        assert!(
            std::fs::symlink_metadata(test_home.join(name)).is_err(),
            "missing links must stay missing when bin did not switch"
        );
    }
}

#[tokio::test]
#[serial]
async fn test_remove_all_versions() {
    let (_tempdir, home_dir) = test_utils::setup_test_environment();
    let test_home = home_dir.join(".wasmedge");

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
#[serial]
async fn test_remove_all_preserves_foreign_files_in_custom_root() {
    let (_tempdir, home_dir) = test_utils::setup_test_environment();
    let test_home = home_dir.join("custom-install-root");

    let version_dir = test_home.join("versions").join("0.14.1");
    setup_mock_version(&version_dir, "0.14.1").await;
    let foreign_file = test_home.join("do-not-delete");
    tokio::fs::write(&foreign_file, "unrelated data")
        .await
        .unwrap();
    #[cfg(unix)]
    let foreign_link = {
        let path = test_home.join("lib");
        std::os::unix::fs::symlink(&foreign_file, &path).unwrap();
        path
    };

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

    assert!(
        foreign_file.exists(),
        "remove --all must preserve files it does not manage"
    );
    #[cfg(unix)]
    assert!(
        std::fs::symlink_metadata(&foreign_link).is_ok(),
        "remove --all must preserve symlinks it does not manage"
    );
    assert!(
        !test_home.join("versions").exists(),
        "remove --all should still remove installed versions"
    );
}

#[tokio::test]
#[serial]
async fn test_remove_all_preserves_unmanaged_versions_entries() {
    let (_tempdir, home_dir) = test_utils::setup_test_environment();
    let test_home = home_dir.join(".wasmedge");

    let version_dir = test_home.join("versions").join("0.14.1");
    setup_mock_version(&version_dir, "0.14.1").await;
    let foreign_file = test_home
        .join("versions")
        .join("backups")
        .join("do-not-delete");
    tokio::fs::create_dir_all(foreign_file.parent().unwrap())
        .await
        .unwrap();
    tokio::fs::write(&foreign_file, "unrelated data")
        .await
        .unwrap();

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

    assert!(
        foreign_file.exists(),
        "remove --all must preserve non-version entries under versions"
    );
    assert!(
        !version_dir.exists(),
        "remove --all should remove semantic-version installation directories"
    );
}

#[tokio::test]
#[serial]
async fn test_remove_all_preserves_unowned_semver_directories() {
    let (_tempdir, home_dir) = test_utils::setup_test_environment();
    let test_home = home_dir.join("custom-install-root");
    let versions_dir = test_home.join("versions");

    let managed_version = versions_dir.join("0.14.1");
    setup_mock_version(&managed_version, "0.14.1").await;

    let foreign_file = versions_dir.join("9.9.9").join("do-not-delete");
    tokio::fs::create_dir_all(foreign_file.parent().unwrap())
        .await
        .unwrap();
    tokio::fs::write(&foreign_file, "unrelated application data")
        .await
        .unwrap();

    RemoveArgs {
        version: String::new(),
        all: true,
        path: Some(test_home),
    }
    .execute(CommandContext {
        client: WasmEdgeApiClient::default(),
        no_progress: true,
    })
    .await
    .unwrap();

    assert!(
        !managed_version.exists(),
        "managed runtime should be removed"
    );
    assert!(
        foreign_file.exists(),
        "a semver-named directory without a WasmEdge payload must remain"
    );
}

#[cfg(unix)]
#[tokio::test]
#[serial]
async fn test_remove_rejects_symlinked_versions_directory() {
    use std::os::unix::fs::symlink;

    let (_tempdir, home_dir) = test_utils::setup_test_environment();
    let test_home = home_dir.join(".wasmedge");
    tokio::fs::create_dir_all(&test_home).await.unwrap();
    let outside = tempfile::tempdir().unwrap();
    let outside_version = outside.path().join("0.14.1");
    tokio::fs::create_dir_all(&outside_version).await.unwrap();
    let sentinel = outside_version.join("do-not-delete");
    tokio::fs::write(&sentinel, "outside install root")
        .await
        .unwrap();
    tokio::fs::create_dir_all(outside.path().join("0.15.0"))
        .await
        .unwrap();
    symlink(outside.path(), test_home.join("versions")).unwrap();

    let remove_args = RemoveArgs {
        version: "0.14.1".to_string(),
        all: false,
        path: Some(test_home),
    };
    let ctx = CommandContext {
        client: WasmEdgeApiClient::default(),
        no_progress: true,
    };
    let result = remove_args.execute(ctx).await;

    assert!(
        matches!(result, Err(Error::InvalidPath { .. })),
        "expected InvalidPath for a symlinked versions directory, got: {result:?}"
    );
    assert!(
        sentinel.exists(),
        "a version outside the install root must not be deleted"
    );
}

#[tokio::test]
#[serial]
async fn test_remove_nonexistent_version() {
    let (_tempdir, home_dir) = test_utils::setup_test_environment();
    let test_home = home_dir.join(".wasmedge");

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

use std::path::Path;

use serial_test::serial;
use wasmedgeup::{
    api::{latest_installed_version, WasmEdgeApiClient},
    cli::{CommandContext, CommandExecutor},
    commands::remove::RemoveArgs,
    constants::{
        VERSION_INSTALL_MARKER, VERSION_INSTALL_MARKER_CONTENT, VERSION_STAGING_MARKER,
        VERSION_STAGING_MARKER_CONTENT,
    },
    error::Error,
    shell_utils,
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
async fn test_remove_current_retargets_all_managed_links_to_replacement() {
    use std::os::unix::fs::symlink;

    let (_tempdir, home_dir) = test_utils::setup_test_environment();
    let test_home = home_dir.join(".wasmedge");
    setup_mock_version(&test_home.join("versions").join("0.14.1"), "0.14.1").await;
    setup_mock_version(&test_home.join("versions").join("0.15.0"), "0.15.0").await;
    setup_mock_version(&test_home.join("versions").join("0.13.0"), "0.13.0").await;

    let include_link = test_home.join("include");
    symlink("versions/0.13.0/include", &include_link).unwrap();

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

    for name in ["bin", "include"] {
        assert_eq!(
            std::fs::read_link(test_home.join(name)).unwrap(),
            Path::new("versions").join("0.15.0").join(name),
            "switching the current version must retarget every managed link"
        );
    }
}

#[cfg(unix)]
#[tokio::test]
#[serial]
async fn test_remove_with_unusable_current_retargets_all_managed_links() {
    use std::os::unix::fs::symlink;

    let (_tempdir, home_dir) = test_utils::setup_test_environment();
    let test_home = home_dir.join(".wasmedge");
    setup_mock_version(&test_home.join("versions/0.13.0"), "0.13.0").await;
    setup_mock_version(&test_home.join("versions/0.14.1"), "0.14.1").await;
    setup_mock_version(&test_home.join("versions/0.15.0"), "0.15.0").await;

    let partial_version = test_home.join("versions/0.16.0");
    tokio::fs::create_dir_all(&partial_version).await.unwrap();
    tokio::fs::write(
        partial_version.join(VERSION_INSTALL_MARKER),
        VERSION_INSTALL_MARKER_CONTENT,
    )
    .await
    .unwrap();

    std::fs::remove_file(test_home.join("bin")).unwrap();
    symlink("versions/0.16.0/bin", test_home.join("bin")).unwrap();
    symlink("versions/0.14.1/include", test_home.join("include")).unwrap();
    symlink("versions/0.13.0/lib", test_home.join("lib")).unwrap();

    RemoveArgs {
        version: "0.13.0".to_string(),
        all: false,
        path: Some(test_home.clone()),
    }
    .execute(CommandContext {
        client: WasmEdgeApiClient::default(),
        no_progress: true,
    })
    .await
    .unwrap();

    assert!(
        partial_version.exists(),
        "the unusable marker-owned version must remain available for explicit cleanup"
    );
    for name in ["bin", "lib", "include", "plugin"] {
        assert_eq!(
            std::fs::read_link(test_home.join(name)).unwrap(),
            Path::new("versions").join("0.15.0").join(name),
            "an unusable current version must trigger a complete managed-link switch"
        );
    }
}

#[cfg(unix)]
#[tokio::test]
#[serial]
async fn test_remove_retargets_stale_links_to_the_current_version() {
    use std::os::unix::fs::symlink;

    let (_tempdir, home_dir) = test_utils::setup_test_environment();
    let test_home = home_dir.join(".wasmedge");
    setup_mock_version(&test_home.join("versions").join("0.14.1"), "0.14.1").await;
    setup_mock_version(&test_home.join("versions").join("0.15.0"), "0.15.0").await;
    setup_mock_version(&test_home.join("versions").join("0.13.0"), "0.13.0").await;

    let include_link = test_home.join("include");
    symlink("versions/0.13.0/include", &include_link).unwrap();

    RemoveArgs {
        version: "0.13.0".to_string(),
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
        Path::new("versions/0.14.1/bin"),
        "the current link must remain on the active version"
    );
    assert_eq!(
        std::fs::read_link(include_link).unwrap(),
        Path::new("versions/0.14.1/include"),
        "stale links must be retargeted to the active version instead of the highest version"
    );
    for name in ["lib", "plugin"] {
        assert!(
            std::fs::symlink_metadata(test_home.join(name)).is_err(),
            "missing links must stay missing when bin did not switch"
        );
    }
}

#[cfg(unix)]
#[tokio::test]
#[serial]
async fn test_remove_equivalent_relative_path_cleans_original_shell_entry() {
    struct CurrentDirGuard(std::path::PathBuf);

    impl Drop for CurrentDirGuard {
        fn drop(&mut self) {
            std::env::set_current_dir(&self.0).unwrap();
        }
    }

    let (_tempdir, home_dir) = test_utils::setup_test_environment();
    let working_dir = tempfile::tempdir().unwrap();
    let _current_dir_guard = CurrentDirGuard(std::env::current_dir().unwrap());
    std::env::set_current_dir(working_dir.path()).unwrap();

    let configured_install = std::path::PathBuf::from("./relative-install");
    let removal_install = std::path::PathBuf::from("relative-install");
    let absolute_install = working_dir.path().join(&removal_install);
    setup_mock_version(&absolute_install.join("versions").join("0.14.1"), "0.14.1").await;
    shell_utils::setup_path(&configured_install).unwrap();

    let profile = home_dir.join(".profile");
    let source_line = r#". "./relative-install/env""#;
    assert!(
        std::fs::read_to_string(&profile)
            .unwrap()
            .contains(source_line),
        "setup must record the caller-provided relative path"
    );

    RemoveArgs {
        version: "0.14.1".to_string(),
        all: false,
        path: Some(removal_install),
    }
    .execute(CommandContext {
        client: WasmEdgeApiClient::default(),
        no_progress: true,
    })
    .await
    .unwrap();

    assert!(
        !std::fs::read_to_string(profile)
            .unwrap()
            .contains(source_line),
        "remove must clean the same relative path spelling written by install"
    );
    assert!(
        !absolute_install.exists(),
        "managed scripts written with an equivalent path spelling must not keep the root alive"
    );
}

#[tokio::test]
#[serial]
async fn test_remove_does_not_retarget_to_marker_only_partial_version() {
    let (_tempdir, home_dir) = test_utils::setup_test_environment();
    let test_home = home_dir.join(".wasmedge");
    let complete_version = test_home.join("versions/0.14.1");
    let partial_version = test_home.join("versions/0.15.0");
    setup_mock_version(&complete_version, "0.14.1").await;
    tokio::fs::create_dir_all(&partial_version).await.unwrap();
    tokio::fs::write(
        partial_version.join(VERSION_INSTALL_MARKER),
        VERSION_INSTALL_MARKER_CONTENT,
    )
    .await
    .unwrap();
    tokio::fs::write(partial_version.join("partial-download"), "incomplete")
        .await
        .unwrap();

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

    assert!(
        partial_version.exists(),
        "the installer-owned partial version must remain available for explicit cleanup"
    );
    assert!(
        std::fs::symlink_metadata(test_home.join("bin")).is_err(),
        "root links must not point at a marker-only partial version"
    );
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
    let foreign_script = {
        let path = test_home.join("env.fish");
        tokio::fs::write(&path, "foreign shell configuration")
            .await
            .unwrap();
        path
    };
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
    assert_eq!(
        tokio::fs::read_to_string(foreign_script).await.unwrap(),
        "foreign shell configuration",
        "remove --all must preserve unowned env scripts"
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
async fn test_remove_all_removes_marker_owned_partial_install() {
    let (_tempdir, home_dir) = test_utils::setup_test_environment();
    let test_home = home_dir.join(".wasmedge");
    let partial_version = test_home.join("versions").join("0.14.1");
    tokio::fs::create_dir_all(&partial_version).await.unwrap();
    tokio::fs::write(
        partial_version.join(VERSION_INSTALL_MARKER),
        VERSION_INSTALL_MARKER_CONTENT,
    )
    .await
    .unwrap();
    tokio::fs::write(partial_version.join("partial-download"), "incomplete")
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
        !partial_version.exists(),
        "an installer-owned partial version must be removable"
    );
}

#[tokio::test]
#[serial]
async fn test_remove_all_removes_interrupted_version_staging() {
    let (_tempdir, home_dir) = test_utils::setup_test_environment();
    let test_home = home_dir.join(".wasmedge");
    let staging_dir = test_home.join("versions").join("interrupted-staging");
    tokio::fs::create_dir_all(staging_dir.join("entry/bin"))
        .await
        .unwrap();
    tokio::fs::write(
        staging_dir.join(VERSION_STAGING_MARKER),
        VERSION_STAGING_MARKER_CONTENT,
    )
    .await
    .unwrap();
    tokio::fs::write(staging_dir.join("entry/bin/partial-runtime"), "partial")
        .await
        .unwrap();

    RemoveArgs {
        version: String::new(),
        all: true,
        path: Some(test_home.clone()),
    }
    .execute(CommandContext {
        client: WasmEdgeApiClient::default(),
        no_progress: true,
    })
    .await
    .unwrap();

    assert!(
        !test_home.exists(),
        "an interrupted installer staging directory must not keep the install root alive"
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

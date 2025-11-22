use std::path::{Path, PathBuf};

use wasmedgeup::{
    api::WasmEdgeApiClient,
    cli::{CommandContext, CommandExecutor},
    commands::plugin::remove::{extract_plugin_name, PluginRemoveArgs},
};

mod test_utils;

fn p(s: &str) -> &Path {
    Path::new(s)
}

#[cfg_attr(not(target_os = "linux"), ignore = "Linux-specific test")]
#[test]
fn test_extract_plugin_name_linux() {
    assert_eq!(
        extract_plugin_name(p("/tmp/libwasmedgePluginwasi_nn.so")).as_deref(),
        Some("wasi_nn")
    );
    assert_eq!(
        extract_plugin_name(p("libwasmedgePluginwasi_logging.so")).as_deref(),
        Some("wasi_logging")
    );
    // wrong prefix
    assert_eq!(extract_plugin_name(p("wasmedgePluginfoo.so")), None);
    // wrong suffix
    assert_eq!(extract_plugin_name(p("libwasmedgePluginfoo.dll")), None);
}

#[cfg_attr(not(target_os = "macos"), ignore = "macOS-specific test")]
#[test]
fn test_extract_plugin_name_macos() {
    assert_eq!(
        extract_plugin_name(p("/tmp/libwasmedgePluginwasi_nn.dylib")).as_deref(),
        Some("wasi_nn")
    );
    assert_eq!(
        extract_plugin_name(p("libwasmedgePluginwasi_logging.dylib")).as_deref(),
        Some("wasi_logging")
    );
    // wrong prefix
    assert_eq!(extract_plugin_name(p("wasmedgePluginfoo.dylib")), None);
    // wrong suffix
    assert_eq!(extract_plugin_name(p("libwasmedgePluginfoo.so")), None);
}

#[cfg_attr(not(target_os = "windows"), ignore = "Windows-specific test")]
#[test]
fn test_extract_plugin_name_windows() {
    assert_eq!(
        extract_plugin_name(p("C:/Temp/wasmedgePluginwasi_nn.dll")).as_deref(),
        Some("wasi_nn")
    );
    assert_eq!(
        extract_plugin_name(p("wasmedgePluginwasi_logging.dll")).as_deref(),
        Some("wasi_logging")
    );
    // wrong prefix
    assert_eq!(extract_plugin_name(p("libwasmedgePluginfoo.dll")), None);
    // wrong suffix
    assert_eq!(extract_plugin_name(p("wasmedgePluginfoo.dylib")), None);
}

fn plugin_filename_for(name: &str) -> String {
    #[cfg(target_os = "linux")]
    {
        format!("libwasmedgePlugin{name}.so")
    }
    #[cfg(target_os = "macos")]
    {
        format!("libwasmedgePlugin{name}.dylib")
    }
    #[cfg(target_os = "windows")]
    {
        format!("wasmedgePlugin{name}.dll")
    }
}

async fn setup_mock_runtime_with_plugins(root: &Path, version: &str, plugins: &[&str]) -> PathBuf {
    let plugin_dir = root.join("versions").join(version).join("plugin");
    tokio::fs::create_dir_all(&plugin_dir).await.unwrap();

    for n in plugins {
        let fname = plugin_filename_for(n);
        tokio::fs::write(plugin_dir.join(fname), format!("mock plugin: {n}"))
            .await
            .unwrap();
    }
    plugin_dir
}

#[tokio::test]
async fn test_plugin_remove_single() {
    let (_tmp, home) = test_utils::setup_test_environment();
    let version = "0.14.1";
    let plugin_dir =
        setup_mock_runtime_with_plugins(&home, version, &["wasi_nn", "wasi_logging"]).await;

    let args = PluginRemoveArgs {
        plugins: vec!["wasi_nn".parse().unwrap()],
        runtime: Some(version.to_string()),
        path: Some(home.clone()),
    };
    let ctx = CommandContext {
        client: WasmEdgeApiClient::default(),
        no_progress: true,
    };
    args.execute(ctx).await.unwrap();

    assert!(!plugin_dir.join(plugin_filename_for("wasi_nn")).exists());
    assert!(plugin_dir
        .join(plugin_filename_for("wasi_logging"))
        .exists());
}

#[tokio::test]
async fn test_plugin_remove_multiple_and_cleanup_empty_dir() {
    let (_tmp, home) = test_utils::setup_test_environment();
    let version = "0.15.0";
    let plugin_dir =
        setup_mock_runtime_with_plugins(&home, version, &["wasi_nn", "wasi_logging"]).await;

    let args = PluginRemoveArgs {
        plugins: vec!["wasi_nn".parse().unwrap(), "wasi_logging".parse().unwrap()],
        runtime: Some(version.to_string()),
        path: Some(home.clone()),
    };
    let ctx = CommandContext {
        client: WasmEdgeApiClient::default(),
        no_progress: true,
    };
    args.execute(ctx).await.unwrap();

    if plugin_dir.exists() {
        let mut entries = tokio::fs::read_dir(&plugin_dir).await.unwrap();
        let mut any = false;
        while let Some(e) = entries.next_entry().await.unwrap() {
            if e.file_type().await.unwrap().is_file() {
                any = true;
                break;
            }
        }
        assert!(!any, "plugin dir should be empty");
    }
}

#[tokio::test]
async fn test_plugin_remove_nonexistent_is_noop() {
    let (_tmp, home) = test_utils::setup_test_environment();
    let version = "0.14.1";
    let plugin_dir = setup_mock_runtime_with_plugins(&home, version, &["wasi_nn"]).await;

    let args = PluginRemoveArgs {
        plugins: vec!["not_exists".parse().unwrap()],
        runtime: Some(version.to_string()),
        path: Some(home.clone()),
    };
    let ctx = CommandContext {
        client: WasmEdgeApiClient::default(),
        no_progress: true,
    };
    args.execute(ctx).await.unwrap();

    assert!(plugin_dir.join(plugin_filename_for("wasi_nn")).exists());
}

#[tokio::test]
async fn test_plugin_remove_when_no_plugin_dir() {
    let (_tmp, home) = test_utils::setup_test_environment();
    let version = "0.14.1";
    let version_dir = home.join("versions").join(version);
    tokio::fs::create_dir_all(&version_dir).await.unwrap();

    let args = PluginRemoveArgs {
        plugins: vec!["wasi_nn".parse().unwrap()],
        runtime: Some(version.to_string()),
        path: Some(home.clone()),
    };
    let ctx = CommandContext {
        client: WasmEdgeApiClient::default(),
        no_progress: true,
    };
    args.execute(ctx).await.unwrap();

    assert!(
        !version_dir.join("plugin").exists(),
        "no plugin dir should be created by remove"
    );
}

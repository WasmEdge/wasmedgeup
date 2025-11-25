use std::path::{Path, PathBuf};
use walkdir::WalkDir;

/// Platform-specific plugin file extension.
pub fn plugin_extension() -> &'static str {
    #[cfg(target_os = "linux")]
    {
        "so"
    }
    #[cfg(target_os = "macos")]
    {
        "dylib"
    }
    #[cfg(target_os = "windows")]
    {
        "dll"
    }
}

/// Platform-specific plugin filename prefix.
pub fn plugin_prefix() -> &'static str {
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    {
        "libwasmedgePlugin"
    }
    #[cfg(target_os = "windows")]
    {
        "wasmedgePlugin"
    }
}

/// Constructs the expected filename for a plugin.
#[allow(dead_code)]
pub fn plugin_filename(name: &str) -> String {
    format!("{}{}.{}", plugin_prefix(), name, plugin_extension())
}

/// Checks if a filename matches the plugin naming convention.
pub fn is_plugin_file(filename: &str) -> bool {
    filename.starts_with(plugin_prefix()) && filename.ends_with(&format!(".{}", plugin_extension()))
}

/// Extracts the plugin name from a plugin filename.
///
/// Returns None if the filename doesn't match the expected plugin pattern.
pub fn extract_plugin_name(path: &Path) -> Option<String> {
    let fname = path.file_name()?.to_str()?;
    let prefix = plugin_prefix();
    let suffix = format!(".{}", plugin_extension());

    fname
        .strip_prefix(prefix)
        .and_then(|rest| rest.strip_suffix(&suffix))
        .map(|core| core.to_string())
}

/// Recursively scans a directory for plugin shared objects.
///
/// Patterns per platform:
/// - Linux: files matching `libwasmedgePlugin*.so`
/// - macOS: files matching `libwasmedgePlugin*.dylib`
/// - Windows: files matching `wasmedgePlugin*.dll`
///
/// Notes:
/// - Ignores the `__MACOSX` metadata directory
/// - Returns a list of absolute paths to matching files.
pub fn find_plugin_shared_objects(root: &Path) -> Vec<PathBuf> {
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
        if is_plugin_file(fname) {
            results.push(path.to_path_buf());
        }
    }
    results
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_plugin_filename_generation() {
        let name = plugin_filename("wasi_nn");
        #[cfg(target_os = "linux")]
        assert_eq!(name, "libwasmedgePluginwasi_nn.so");
        #[cfg(target_os = "macos")]
        assert_eq!(name, "libwasmedgePluginwasi_nn.dylib");
        #[cfg(target_os = "windows")]
        assert_eq!(name, "wasmedgePluginwasi_nn.dll");
    }

    #[test]
    fn test_is_plugin_file_positive() {
        #[cfg(target_os = "linux")]
        assert!(is_plugin_file("libwasmedgePlugintest.so"));
        #[cfg(target_os = "macos")]
        assert!(is_plugin_file("libwasmedgePlugintest.dylib"));
        #[cfg(target_os = "windows")]
        assert!(is_plugin_file("wasmedgePlugintest.dll"));
    }

    #[test]
    fn test_is_plugin_file_negative() {
        assert!(!is_plugin_file("random_file.txt"));
        assert!(!is_plugin_file("libsomething.so"));
        assert!(!is_plugin_file("wasmedge.dll"));
    }

    #[test]
    fn test_extract_plugin_name() {
        #[cfg(target_os = "linux")]
        {
            let path = Path::new("/some/path/libwasmedgePluginwasi_nn.so");
            assert_eq!(extract_plugin_name(path), Some("wasi_nn".to_string()));
        }
        #[cfg(target_os = "macos")]
        {
            let path = Path::new("/some/path/libwasmedgePluginwasi_nn.dylib");
            assert_eq!(extract_plugin_name(path), Some("wasi_nn".to_string()));
        }
        #[cfg(target_os = "windows")]
        {
            let path = Path::new("C:\\some\\path\\wasmedgePluginwasi_nn.dll");
            assert_eq!(extract_plugin_name(path), Some("wasi_nn".to_string()));
        }
    }

    #[test]
    fn test_extract_plugin_name_non_plugin() {
        let path = Path::new("/some/path/random_file.txt");
        assert_eq!(extract_plugin_name(path), None);
    }
}

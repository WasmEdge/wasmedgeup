use std::path::PathBuf;

pub fn setup_test_environment() -> (tempfile::TempDir, PathBuf) {
    let test_home = tempfile::tempdir().unwrap();
    let test_home_path = test_home.path().to_path_buf();

    // Set up environment variables based on platform
    #[cfg(windows)]
    {
        std::env::set_var("USERPROFILE", &test_home_path);
    }
    #[cfg(unix)]
    {
        std::env::set_var("HOME", &test_home_path);
        std::env::set_var("ZDOTDIR", &test_home_path);

        let zshenv_path = test_home_path.join(".zshenv");
        if let Some(parent) = zshenv_path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(&zshenv_path, "").unwrap();
    }

    (test_home, test_home_path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[cfg(unix)]
    fn test_unix_environment_isolation() {
        let real_home = std::env::var("HOME").unwrap();
        let real_zdotdir = std::env::var("ZDOTDIR").unwrap_or_else(|_| real_home.clone());

        let (_tempdir, test_home) = setup_test_environment();

        let new_home = std::env::var("HOME").unwrap();
        assert_eq!(
            test_home.to_string_lossy().to_string(),
            new_home,
            "HOME should be set to test directory"
        );
        assert_ne!(
            new_home, real_home,
            "Test HOME should be different from real HOME"
        );

        let new_zdotdir = std::env::var("ZDOTDIR").unwrap();
        assert_eq!(new_home, new_zdotdir, "ZDOTDIR should match test HOME");
        assert_ne!(
            new_zdotdir, real_zdotdir,
            "Test ZDOTDIR should be different from real ZDOTDIR"
        );

        let zshenv_path = PathBuf::from(&new_home).join(".zshenv");
        assert!(
            zshenv_path.exists(),
            ".zshenv should exist in test directory"
        );
    }

    #[test]
    #[cfg(windows)]
    fn test_windows_environment_isolation() {
        let real_profile = std::env::var("USERPROFILE").unwrap();

        let (_tempdir, test_home) = setup_test_environment();

        let new_profile = std::env::var("USERPROFILE").unwrap();
        assert_eq!(
            test_home.to_string_lossy().to_string(),
            new_profile,
            "USERPROFILE should be set to test directory"
        );
        assert_ne!(
            new_profile, real_profile,
            "Test USERPROFILE should be different from real USERPROFILE"
        );
    }
}

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

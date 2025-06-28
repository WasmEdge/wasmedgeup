use crate::error::{IoSnafu, Result};
use std::path::{Path, PathBuf};
use tokio::fs::OpenOptions;
use tokio::io::AsyncWriteExt;
use snafu::ResultExt;

// Struct to hold information about a shell-specific environment script
#[derive(Debug, Clone, Copy)]
pub struct ShellScript {
    pub template: &'static str,
    pub name: &'static str,
}

// Trait for defining behavior for different Unix shells
pub trait UnixShell: Send + Sync {
    fn name(&self) -> &'static str;
    fn is_present(&self, home_dir: &Path) -> bool;

    // Renamed from rc_files. This might list multiple potential standard locations.
    // For shells like Nushell, it lists a few possibilities.
    // For others, it might just be one.
    fn potential_rc_paths(&self, home_dir: &Path) -> Vec<PathBuf>;

    // NEW: Returns the single, definitive rc file path to be modified.
    // This will incorporate ZDOTDIR logic for Zsh and target .zshenv.
    // For others, it will point to their primary rc file.
    // Returns None if no single effective path is determined (e.g. Nushell might need to check existence).
    fn effective_rc_file(&self, home_dir: &Path) -> Option<PathBuf>;

    fn env_script(&self) -> ShellScript;
    fn source_line(&self, script_path: &Path) -> String;
}

// --- Shell Implementations ---

// Posix (sh) Implementation
#[derive(Debug, Default)]
pub struct Posix;
impl UnixShell for Posix {
    fn name(&self) -> &'static str { "sh" }
    fn is_present(&self, _home_dir: &Path) -> bool {
        // Assume POSIX sh is always a possibility to configure for via .profile.
        // The actual update to .profile will only happen if it exists.
        true
    }
    fn potential_rc_paths(&self, home_dir: &Path) -> Vec<PathBuf> { vec![home_dir.join(".profile")] }
    fn effective_rc_file(&self, home_dir: &Path) -> Option<PathBuf> { Some(home_dir.join(".profile")) }
    fn env_script(&self) -> ShellScript {
        ShellScript { template: include_str!("env.sh"), name: "env" }
    }
    fn source_line(&self, script_path: &Path) -> String {
        format!(
            "if [ -f \"{0}\" ]; then . \"{0}\"; fi # WasmEdge env",
            script_path.to_string_lossy()
        )
    }
}

// Bash Implementation
#[derive(Debug, Default)]
pub struct Bash;
impl UnixShell for Bash {
    fn name(&self) -> &'static str { "bash" }
    fn is_present(&self, home_dir: &Path) -> bool {
        // Only configure if .bashrc exists.
        self.potential_rc_paths(home_dir).iter().any(|f| f.exists())
    }
    fn potential_rc_paths(&self, home_dir: &Path) -> Vec<PathBuf> { vec![home_dir.join(".bashrc")] }
    fn effective_rc_file(&self, home_dir: &Path) -> Option<PathBuf> { Some(home_dir.join(".bashrc")) }
    fn env_script(&self) -> ShellScript {
        ShellScript { template: include_str!("env.sh"), name: "env" }
    }
    fn source_line(&self, script_path: &Path) -> String {
        format!("source \"{}\"", script_path.to_string_lossy())
    }
}

// Zsh Implementation
#[derive(Debug, Default)]
pub struct Zsh;
impl UnixShell for Zsh {
    fn name(&self) -> &'static str { "zsh" }
    fn is_present(&self, _home_dir: &Path) -> bool {
        matches!(std::env::var("SHELL"), Ok(sh) if sh.ends_with("/zsh"))
            || is_command_in_path("zsh")
    }

    // Helper to determine the Zsh configuration directory ($ZDOTDIR or $HOME)
    fn determine_config_dir(&self, home_dir: &Path) -> PathBuf {
        // 1. Try invoking zsh to get ZDOTDIR
        if let Ok(output) = std::process::Command::new("zsh")
            .arg("-ic") // -i for interactive, -c for command. Ensures sourcing of zshenv etc.
            .arg("echo $ZDOTDIR")
            .output()
        {
            if output.status.success() {
                let zdotdir_str = String::from_utf8_lossy(&output.stdout).trim().to_string();
                if !zdotdir_str.is_empty() {
                    let zdotdir_path = PathBuf::from(zdotdir_str);
                    if zdotdir_path.is_dir() {
                        return zdotdir_path;
                    }
                }
            }
        }

        // 2. Fallback: Check ZDOTDIR environment variable directly
        if let Ok(zdotdir_env_str) = std::env::var("ZDOTDIR") {
            let zdotdir_env_path = PathBuf::from(zdotdir_env_str);
            if zdotdir_env_path.is_dir() {
                return zdotdir_env_path;
            }
        }

        // 3. Default to $HOME
        home_dir.to_path_buf()
    }

    // Internal helper to list potential .zshenv paths, ZDOTDIR-aware first, then $HOME.
    fn internal_potential_zshenv_paths(&self, home_dir: &Path) -> Vec<PathBuf> {
        let mut paths = Vec::new();
        let zdotdir_config_path = self.determine_config_dir(home_dir).join(".zshenv");
        paths.push(zdotdir_config_path.clone());

        let home_zshenv_path = home_dir.join(".zshenv");
        // Add $HOME/.zshenv only if it's different from the ZDOTDIR path
        if zdotdir_config_path != home_zshenv_path {
            paths.push(home_zshenv_path);
        }
        paths
    }

    // This is the trait method
    fn potential_rc_paths(&self, home_dir: &Path) -> Vec<PathBuf> {
        self.internal_potential_zshenv_paths(home_dir)
    }

    fn effective_rc_file(&self, home_dir: &Path) -> Option<PathBuf> {
        let potential_paths = self.internal_potential_zshenv_paths(home_dir);

        // Try to find an existing .zshenv file from the potential paths
        for path in &potential_paths {
            if path.is_file() { // is_file also implies exists()
                return Some(path.clone());
            }
        }

        // If no existing .zshenv is found, default to the first potential path
        // (which is ZDOTDIR-aware, or $HOME/.zshenv if ZDOTDIR isn't set/valid).
        // This path will be used for creation.
        potential_paths.into_iter().next()
    }

    fn env_script(&self) -> ShellScript {
        ShellScript { template: include_str!("env.sh"), name: "env" }
    }
    fn source_line(&self, script_path: &Path) -> String {
        format!("source \"{}\"", script_path.to_string_lossy())
    }
}

// Fish Implementation
#[derive(Debug, Default)]
pub struct Fish;
impl UnixShell for Fish {
    fn name(&self) -> &'static str { "fish" }
    fn is_present(&self, _home_dir: &Path) -> bool {
        matches!(std::env::var("SHELL"), Ok(sh) if sh.ends_with("/fish"))
            || is_command_in_path("fish")
    }
    fn potential_rc_paths(&self, home_dir: &Path) -> Vec<PathBuf> { vec![home_dir.join(".config/fish/config.fish")] }
    fn effective_rc_file(&self, home_dir: &Path) -> Option<PathBuf> { Some(home_dir.join(".config/fish/config.fish")) }
    fn env_script(&self) -> ShellScript {
        ShellScript { template: include_str!("env.fish"), name: "env.fish" }
    }
    fn source_line(&self, script_path: &Path) -> String {
        format!("source {}", script_path.to_string_lossy())
    }
}

// Nushell Implementation
#[derive(Debug, Default)]
pub struct Nushell;
impl UnixShell for Nushell {
    fn name(&self) -> &'static str { "nushell" }
    fn is_present(&self, _home_dir: &Path) -> bool {
        matches!(std::env::var("SHELL"), Ok(sh) if sh.ends_with("/nu"))
            || is_command_in_path("nu")
    }
    fn potential_rc_paths(&self, _home_dir: &Path) -> Vec<PathBuf> {
        let mut paths = Vec::new();
        if let Some(conf_dir) = dirs::config_dir() {
            paths.push(conf_dir.join("nushell/config.nu"));
            paths.push(conf_dir.join("nu/config.nu")); // Older path
        }
        // Could also add XDG_CONFIG_HOME based paths if strictly necessary,
        // but dirs::config_dir() usually covers the standard cases.
        paths
    }
    fn effective_rc_file(&self, _home_dir: &Path) -> Option<PathBuf> {
        if let Some(conf_dir) = dirs::config_dir() {
            let primary_path = conf_dir.join("nushell/config.nu");
            if primary_path.exists() {
                return Some(primary_path);
            }
            let secondary_path = conf_dir.join("nu/config.nu"); // Older path
            if secondary_path.exists() {
                return Some(secondary_path);
            }
        }
        None // No existing config file found at standard locations
    }
    fn env_script(&self) -> ShellScript {
        ShellScript { template: include_str!("env.nu"), name: "env.nu" }
    }
    fn source_line(&self, script_path: &Path) -> String {
        format!("source-env \"{}\"", script_path.to_string_lossy())
    }
}

// --- Helper Functions & Embedded Content ---

// Helper function to check if a command exists in the system PATH
pub fn is_command_in_path(command_name: &str) -> bool {
    if let Ok(path_var) = std::env::var("PATH") {
        for path_dir in std::env::split_paths(&path_var) {
            let command_path = path_dir.join(command_name);
            if command_path.is_file() { // Could also check for executable permissions if needed
                return true;
            }
        }
    }
    false
}

pub fn get_supported_shells() -> Vec<Box<dyn UnixShell>> {
    vec![
        Box::new(Posix::default()), // Added Posix
        Box::new(Bash::default()),
        Box::new(Zsh::default()),
        Box::new(Fish::default()),
        Box::new(Nushell::default()),
    ]
}

pub async fn append_to_file_if_not_present(file_path: &Path, line: &str) -> Result<()> {
    let file_content = tokio::fs::read_to_string(file_path).await.ok();
    if file_content.map_or(true, |content| !content.contains(line)) {
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(file_path)
            .await
            .context(IoSnafu)?;
        file.write_all(format!("\n{}", line).as_bytes())
            .await
            .context(IoSnafu)?;
    }
    Ok(())
}

// --- Tests ---
#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    // Tests for script content (get_env_X_content) are removed as content is now in include_str!
    // within trait impls' env_script() method. The logic of those scripts is implicitly
    // tested by checking `shell.env_script().template == include_str!("env.sh")` etc.
    // in the specific shell trait implementation tests.

    // Test for append_to_file_if_not_present
    #[tokio::test]
    async fn test_append_to_file_if_not_present_logic() -> crate::error::Result<()> {
        let dir = tempdir().unwrap();
        let file_path = dir.path().join("test_append.txt");
        let line1 = "FIRST LINE";
        let line2 = "SECOND LINE";

        append_to_file_if_not_present(&file_path, line1).await?;
        let content1 = tokio::fs::read_to_string(&file_path).await?;
        assert!(content1.contains(line1));

        append_to_file_if_not_present(&file_path, line1).await?; // Add same line
        let content2 = tokio::fs::read_to_string(&file_path).await?;
        assert_eq!(content1.matches(line1).count(), 1, "Line should only appear once");

        append_to_file_if_not_present(&file_path, line2).await?;
        let content3 = tokio::fs::read_to_string(&file_path).await?;
        assert!(content3.contains(line1));
        assert!(content3.contains(line2));
        Ok(())
    }

    // --- Tests for UnixShell Trait Implementations ---
    #[test]
    fn test_posix_methods() {
        let shell = Posix::default();
        let temp_home = tempdir().unwrap();
        // .profile may or may not exist, is_present should be true regardless
        // std::fs::File::create(temp_home.path().join(".profile")).unwrap();

        assert_eq!(shell.name(), "sh");
        assert!(shell.is_present(temp_home.path()), "Posix::is_present should always be true");
        assert_eq!(shell.potential_rc_paths(temp_home.path()), vec![temp_home.path().join(".profile")]);
        assert_eq!(shell.effective_rc_file(temp_home.path()), Some(temp_home.path().join(".profile")));

        let script_details = shell.env_script();
        assert_eq!(script_details.name, "env");
        assert_eq!(script_details.template, include_str!("env.sh"));

        let dummy_script_path = Path::new("/test/env");
        assert_eq!(
            shell.source_line(dummy_script_path),
            "if [ -f \"/test/env\" ]; then . \"/test/env\"; fi # WasmEdge env"
        );
    }

    #[test]
    fn test_bash_methods() {
        let shell = Bash::default();
        let temp_home = tempdir().unwrap();

        assert_eq!(shell.name(), "bash");
        // Test is_present when .bashrc does NOT exist
        assert!(!shell.is_present(temp_home.path()), "Bash::is_present should be false if .bashrc does not exist");

        std::fs::File::create(temp_home.path().join(".bashrc")).unwrap();
        // Test is_present when .bashrc DOES exist
        assert!(shell.is_present(temp_home.path()), "Bash::is_present should be true if .bashrc exists");

        assert_eq!(shell.potential_rc_paths(temp_home.path()), vec![temp_home.path().join(".bashrc")]);
        assert_eq!(shell.effective_rc_file(temp_home.path()), Some(temp_home.path().join(".bashrc")));

        let script_details = shell.env_script();
        assert_eq!(script_details.name, "env"); // Updated expected name
        assert_eq!(script_details.template, include_str!("env.sh"));

        let dummy_script_path = Path::new("/some/path/env"); // Updated path
        assert_eq!(shell.source_line(dummy_script_path), "source \"/some/path/env\"");
    }

    #[test]
    fn test_zsh_methods() {
        let shell = Zsh::default();
        let temp_home = tempdir().unwrap();
        let home_path = temp_home.path();

        assert_eq!(shell.name(), "zsh");

        // --- Test effective_rc_file() and potential_rc_paths() logic ---
        // These tests simulate conditions by setting ZDOTDIR env var and creating/deleting files.
        // The zsh command execution for ZDOTDIR is harder to test directly here.

        let zdotdir_dir = temp_home.path().join("my_zdotdir");
        let zdotdir_zshenv = zdotdir_dir.join(".zshenv");
        let home_zshenv = home_path.join(".zshenv");

        // Scenario 1: .zshenv exists in ZDOTDIR (via env var)
        std::fs::create_dir_all(&zdotdir_dir).unwrap();
        std::fs::File::create(&zdotdir_zshenv).unwrap();
        std::env::set_var("ZDOTDIR", zdotdir_dir.to_str().unwrap());
        assert_eq!(shell.effective_rc_file(home_path), Some(zdotdir_zshenv.clone()), "Should find .zshenv in ZDOTDIR");
        std::fs::remove_file(&zdotdir_zshenv).unwrap(); // Clean up file
        std::env::remove_var("ZDOTDIR"); // Clean up env var

        // Scenario 2: No .zshenv in ZDOTDIR (or ZDOTDIR not set/invalid), but .zshenv exists in $HOME
        // Ensure ZDOTDIR is not set or points to a place without .zshenv
        std::fs::File::create(&home_zshenv).unwrap();
        assert_eq!(shell.effective_rc_file(home_path), Some(home_zshenv.clone()), "Should find .zshenv in $HOME");
        std::fs::remove_file(&home_zshenv).unwrap(); // Clean up file

        // Scenario 3: No .zshenv exists in ZDOTDIR (env var set) nor $HOME. Should return ZDOTDIR path for creation.
        std::env::set_var("ZDOTDIR", zdotdir_dir.to_str().unwrap());
        // Ensure no .zshenv files exist
        if zdotdir_zshenv.exists() { std::fs::remove_file(&zdotdir_zshenv).unwrap(); }
        if home_zshenv.exists() { std::fs::remove_file(&home_zshenv).unwrap(); }
        assert_eq!(shell.effective_rc_file(home_path), Some(zdotdir_zshenv.clone()), "Should return ZDOTDIR path for creation if no .zshenv exists");
        std::env::remove_var("ZDOTDIR");

        // Scenario 4: No .zshenv anywhere, ZDOTDIR not set. Should return $HOME path for creation.
        if zdotdir_zshenv.exists() { std::fs::remove_file(&zdotdir_zshenv).unwrap(); }
        if home_zshenv.exists() { std::fs::remove_file(&home_zshenv).unwrap(); }
        assert_eq!(shell.effective_rc_file(home_path), Some(home_zshenv.clone()), "Should return $HOME path for creation if no .zshenv and no ZDOTDIR");

        // Test potential_rc_paths
        std::env::set_var("ZDOTDIR", zdotdir_dir.to_str().unwrap());
        let potential = shell.potential_rc_paths(home_path);
        assert!(potential.contains(&zdotdir_zshenv));
        assert!(potential.contains(&home_zshenv)); // Will be present if different from zdotdir_zshenv
        std::env::remove_var("ZDOTDIR");
        std::fs::remove_dir_all(&zdotdir_dir).unwrap_or_default(); // Clean up directory


        // --- Test is_present() logic ---
        assert!(!shell.is_present(home_path), "Zsh::is_present default in test env");

        // --- Test other methods ---
        let script_details = shell.env_script();
        assert_eq!(script_details.name, "env");
        assert_eq!(script_details.template, include_str!("env.sh"));

        let dummy_script_path = Path::new("/some/path/env"); // Updated path
        assert_eq!(shell.source_line(dummy_script_path), "source \"/some/path/env\"");
    }

    #[test]
    fn test_fish_methods() {
        let shell = Fish::default();
        let temp_home = tempdir().unwrap();
        let home_path = temp_home.path();
        let fish_config_path = home_path.join(".config/fish/config.fish");
        // Note: We don't create the config file for is_present, as it no longer checks it directly

        assert_eq!(shell.name(), "fish");
        // is_present logic for Fish now primarily relies on $SHELL and PATH.
        assert!(!shell.is_present(home_path), "Fish::is_present should be false if not in SHELL or PATH (in this test env)");

        assert_eq!(shell.potential_rc_paths(home_path), vec![fish_config_path.clone()]);
        assert_eq!(shell.effective_rc_file(home_path), Some(fish_config_path.clone()));

        let script_details = shell.env_script();
        assert_eq!(script_details.name, "env.fish");
        assert_eq!(script_details.template, include_str!("env.fish"));

        let dummy_script_path = Path::new("/some/path/env.fish");
        assert_eq!(shell.source_line(dummy_script_path), "source /some/path/env.fish");
    }

    #[test]
    fn test_nushell_methods() {
        let shell = Nushell::default();
        let temp_home = tempdir().unwrap(); // Though home_path isn't used by Nushell's is_present

        assert_eq!(shell.name(), "nushell");
        // is_present logic for Nushell now primarily relies on $SHELL and PATH.
        assert!(!shell.is_present(temp_home.path()), "Nushell::is_present should be false if not in SHELL or PATH (in this test env)");

        let script_details = shell.env_script();
        assert_eq!(script_details.name, "env.nu");
        assert_eq!(script_details.template, include_str!("env.nu"));

        let dummy_script_path = Path::new("/some/path/env.nu");
        assert_eq!(shell.source_line(dummy_script_path), "source-env \"/some/path/env.nu\"");

        // Test effective_rc_file for Nushell
        if let Some(config_dir) = dirs::config_dir() {
            let primary_nu_config = config_dir.join("nushell/config.nu");
            let secondary_nu_config = config_dir.join("nu/config.nu");

            // Scenario 1: No config files exist
            assert_eq!(shell.effective_rc_file(temp_home.path()), None, "Should return None if no Nushell config exists");

            // Scenario 2: Primary exists
            std::fs::create_dir_all(primary_nu_config.parent().unwrap()).unwrap_or_default(); // Ensure parent dir
            std::fs::File::create(&primary_nu_config).unwrap();
            assert_eq!(shell.effective_rc_file(temp_home.path()), Some(primary_nu_config.clone()), "Should find primary Nushell config");
            std::fs::remove_file(&primary_nu_config).unwrap(); // Clean up

            // Scenario 3: Secondary exists, primary does not
            std::fs::create_dir_all(secondary_nu_config.parent().unwrap()).unwrap_or_default(); // Ensure parent dir
            std::fs::File::create(&secondary_nu_config).unwrap();
            assert_eq!(shell.effective_rc_file(temp_home.path()), Some(secondary_nu_config.clone()), "Should find secondary Nushell config");
            std::fs::remove_file(&secondary_nu_config).unwrap(); // Clean up

            // Check potential_rc_paths
            let expected_potential_paths = vec![primary_nu_config, secondary_nu_config];
            assert_eq!(shell.potential_rc_paths(Path::new("dummy_home")), expected_potential_paths);
        }
    }

    #[test]
    fn test_get_supported_shells_list() {
        let shells = get_supported_shells();
        assert_eq!(shells.len(), 5); // Posix, Bash, Zsh, Fish, Nushell
        assert!(shells.iter().any(|s| s.name() == "sh"));
        assert!(shells.iter().any(|s| s.name() == "bash"));
        assert!(shells.iter().any(|s| s.name() == "zsh"));
        assert!(shells.iter().any(|s| s.name() == "fish"));
        assert!(shells.iter().any(|s| s.name() == "nushell"));
    }
    // Old source_env_file tests removed as their logic is covered by trait method tests
    // and append_to_file_if_not_present test.

    #[cfg(test)]
    mod command_in_path_tests {
        use super::is_command_in_path;
        use std::env;
        use std::fs::{self, File};
        use std::os::unix::fs::PermissionsExt; // For setting executable bit
        use tempfile::tempdir;

        #[test]
        fn test_is_command_in_path_found() {
            let dir = tempdir().unwrap();
            let bin_dir = dir.path().join("bin");
            fs::create_dir(&bin_dir).unwrap();
            let cmd_path = bin_dir.join("test_cmd");
            File::create(&cmd_path).unwrap();
            // Set executable permission (important for some OS behavior, though is_file() might not strictly check it)
            let mut perms = fs::metadata(&cmd_path).unwrap().permissions();
            perms.set_mode(0o755); // rwxr-xr-x
            fs::set_permissions(&cmd_path, perms).unwrap();

            let original_path = env::var("PATH").unwrap_or_default();
            let test_path = format!("{}:{}", bin_dir.to_str().unwrap(), original_path);

            env::set_var("PATH", &test_path);
            assert!(is_command_in_path("test_cmd"));
            env::set_var("PATH", original_path); // Restore original PATH
        }

        #[test]
        fn test_is_command_in_path_not_found() {
            let dir = tempdir().unwrap();
            let other_dir = dir.path().join("other");
            fs::create_dir(&other_dir).unwrap();
            // test_cmd_missing is not in other_dir or default PATH

            let original_path = env::var("PATH").unwrap_or_default();
            let test_path = format!("{}:{}", other_dir.to_str().unwrap(), original_path);

            env::set_var("PATH", &test_path);
            assert!(!is_command_in_path("test_cmd_missing"));
            env::set_var("PATH", original_path); // Restore original PATH
        }

        #[test]
        fn test_is_command_in_path_empty_path_var() {
            let original_path = env::var_os("PATH");
            env::remove_var("PATH"); // Simulate no PATH variable
            assert!(!is_command_in_path("any_cmd"));
            if let Some(path) = original_path {
                env::set_var("PATH", path); // Restore
            }
        }
         #[test]
        fn test_is_command_in_path_cmd_is_dir() {
            let dir = tempdir().unwrap();
            let bin_dir = dir.path().join("bin");
            fs::create_dir(&bin_dir).unwrap();
            let cmd_as_dir_path = bin_dir.join("test_cmd_is_dir");
            fs::create_dir(&cmd_as_dir_path).unwrap(); // Create a directory with the command name

            let original_path = env::var("PATH").unwrap_or_default();
            let test_path = format!("{}:{}", bin_dir.to_str().unwrap(), original_path);

            env::set_var("PATH", &test_path);
            assert!(!is_command_in_path("test_cmd_is_dir"), "Should not find command if it's a directory");
            env::set_var("PATH", original_path);
        }
    }
}

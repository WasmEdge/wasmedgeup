use serial_test::serial;
use std::env;
use tempfile::TempDir;
use wasmedgeup::prelude::*;
use wasmedgeup::shell_utils;

mod test_utils;
use test_utils::setup_test_environment;

#[test]
#[serial]
fn test_linux_library_path() {
    if cfg!(target_os = "linux") {
        let (_test_home, _home_path) = setup_test_environment();
        let temp_dir = tempfile::tempdir().unwrap();
        let install_dir = temp_dir.path().to_path_buf();

        std::fs::create_dir_all(install_dir.join("bin")).unwrap();
        std::fs::create_dir_all(install_dir.join("lib")).unwrap();

        env::remove_var("LD_LIBRARY_PATH");

        shell_utils::setup_path(&install_dir).unwrap();

        let env_content = std::fs::read_to_string(install_dir.join("env")).unwrap();
        assert!(env_content.contains("LD_LIBRARY_PATH"));
        assert!(env_content.contains(&format!("{}/{}", install_dir.display(), LIB_DIR)));

        println!("Env file contents:\n{env_content}");

        let output = std::process::Command::new("bash")
            .arg("-c")
            .arg(format!("uname() {{ echo Linux; }}; export -f uname; export LD_LIBRARY_PATH='' && source {} && echo $LD_LIBRARY_PATH", install_dir.join("env").display()))
            .output()
            .unwrap();

        let lib_path = String::from_utf8_lossy(&output.stdout);
        println!("LD_LIBRARY_PATH: {lib_path}");
        println!("Expected path: {}/{}", install_dir.display(), LIB_DIR);
        assert!(lib_path.contains(&format!("{}/{}", install_dir.display(), LIB_DIR)));
    }
}

#[test]
#[serial]
fn test_macos_library_path() {
    if cfg!(target_os = "macos") {
        let (_test_home, _home_path) = setup_test_environment();
        let temp_dir = tempfile::tempdir().unwrap();
        let install_dir = temp_dir.path().to_path_buf();

        std::fs::create_dir_all(install_dir.join("bin")).unwrap();
        std::fs::create_dir_all(install_dir.join("lib")).unwrap();

        env::remove_var("DYLD_LIBRARY_PATH");

        shell_utils::setup_path(&install_dir).unwrap();

        let env_content = std::fs::read_to_string(install_dir.join("env")).unwrap();
        assert!(env_content.contains("DYLD_LIBRARY_PATH"));
        assert!(env_content.contains(&format!("{}/{}", install_dir.display(), LIB_DIR)));

        let output = std::process::Command::new("bash")
            .arg("-c")
            .arg(format!("uname() {{ echo Darwin; }}; export -f uname; export DYLD_LIBRARY_PATH='' && source {} && echo $DYLD_LIBRARY_PATH", install_dir.join("env").display()))
            .output()
            .unwrap();

        let lib_path = String::from_utf8_lossy(&output.stdout);
        assert!(lib_path.contains(&format!("{}/{}", install_dir.display(), LIB_DIR)));
    }
}

#[test]
#[serial]
fn test_library_path_append() {
    let (_test_home, _home_path) = setup_test_environment();
    let temp_dir = TempDir::new().unwrap();
    let install_dir = temp_dir.path().to_path_buf();

    std::fs::create_dir_all(install_dir.join("bin")).unwrap();
    std::fs::create_dir_all(install_dir.join("lib")).unwrap();

    let existing_path = "/existing/path";

    shell_utils::setup_path(&install_dir).unwrap();

    if cfg!(target_os = "linux") {
        let output = std::process::Command::new("bash")
            .arg("-c")
            .arg(format!(
                "uname() {{ echo Linux; }}; export -f uname; export LD_LIBRARY_PATH='{}' && source {} && echo $LD_LIBRARY_PATH",
                existing_path,
                install_dir.join("env").display()
            ))
            .output()
            .unwrap();

        let lib_path = String::from_utf8_lossy(&output.stdout);
        assert!(lib_path.contains(&format!("{}/{}", install_dir.display(), LIB_DIR)));
        assert!(lib_path.contains(existing_path));
    } else if cfg!(target_os = "macos") {
        let output = std::process::Command::new("bash")
            .arg("-c")
            .arg(format!(
                "uname() {{ echo Darwin; }}; export -f uname; export DYLD_LIBRARY_PATH='{}' && source {} && echo $DYLD_LIBRARY_PATH",
                existing_path,
                install_dir.join("env").display()
            ))
            .output()
            .unwrap();

        let lib_path = String::from_utf8_lossy(&output.stdout);
        assert!(lib_path.contains(&format!("{}/{}", install_dir.display(), LIB_DIR)));
        assert!(lib_path.contains(existing_path));
    }
}

#[cfg(all(test, unix))]
mod setup_uninstall {
    use super::setup_test_environment;
    use serial_test::serial;
    use std::fs;
    use wasmedgeup::prelude::*;
    use wasmedgeup::shell_utils;

    #[test]
    #[serial]
    fn test_no_rc_file() {
        let (_tmp_home, home) = setup_test_environment();

        let zshenv = home.join(".zshenv");
        if zshenv.exists() {
            fs::remove_file(&zshenv).unwrap();
        }

        let install_dir = tempfile::tempdir().unwrap();
        fs::create_dir_all(install_dir.path().join("bin")).unwrap();
        fs::create_dir_all(install_dir.path().join("lib")).unwrap();

        std::env::set_var("SHELL", "/bin/zsh");

        shell_utils::setup_path(install_dir.path()).unwrap();

        let expected_source = format!(r#". "{}/env""#, install_dir.path().display());
        let mut any_contains = false;
        for shell in shell_utils::get_available_shells() {
            for rc in shell.effective_rc_files() {
                if rc.exists() {
                    if let Ok(content) = std::fs::read_to_string(&rc) {
                        if content.contains(&expected_source) {
                            any_contains = true;
                        }
                    }
                }
            }
        }
        assert!(
            any_contains,
            "expected at least one rc file to contain the source line"
        );

        let env_path = install_dir.path().join("env");
        assert!(env_path.exists());
        let env_content = fs::read_to_string(&env_path).unwrap();
        assert!(env_content.contains(&format!("{}/{}", install_dir.path().display(), LIB_DIR)));

        shell_utils::uninstall_path(install_dir.path()).unwrap();

        for shell in shell_utils::get_available_shells() {
            for rc in shell.effective_rc_files() {
                if rc.exists() {
                    let rc_after = std::fs::read_to_string(&rc).unwrap_or_default();
                    assert!(!rc_after.contains(&expected_source));
                }
            }
        }
        assert!(!env_path.exists());
    }

    #[test]
    #[serial]
    fn test_empty_rc_file() {
        let (_tmp_home, home) = setup_test_environment();

        let zshenv = home.join(".zshenv");
        fs::write(&zshenv, "").unwrap();

        let install_dir = tempfile::tempdir().unwrap();
        fs::create_dir_all(install_dir.path().join("bin")).unwrap();
        fs::create_dir_all(install_dir.path().join("lib")).unwrap();

        std::env::set_var("SHELL", "/bin/zsh");

        shell_utils::setup_path(install_dir.path()).unwrap();

        let expected_source = format!(r#". "{}/env""#, install_dir.path().display());
        let mut any_contains = false;
        for shell in shell_utils::get_available_shells() {
            for rc in shell.effective_rc_files() {
                if rc.exists() {
                    if let Ok(content) = std::fs::read_to_string(&rc) {
                        if content.contains(&expected_source) {
                            any_contains = true;
                        }
                    }
                }
            }
        }
        assert!(
            any_contains,
            "expected at least one rc file to contain the source line"
        );

        shell_utils::uninstall_path(install_dir.path()).unwrap();
        for shell in shell_utils::get_available_shells() {
            for rc in shell.effective_rc_files() {
                if rc.exists() {
                    let rc_after = std::fs::read_to_string(&rc).unwrap_or_default();
                    assert!(!rc_after.contains(&expected_source));
                }
            }
        }
    }

    #[test]
    #[serial]
    fn test_rc_with_existing_content() {
        let (_tmp_home, home) = setup_test_environment();

        let zshenv = home.join(".zshenv");
        let existing = "# existing config\nexport PATH=\"$PATH:/some/where\"";
        fs::write(&zshenv, existing).unwrap();

        let install_dir = tempfile::tempdir().unwrap();
        fs::create_dir_all(install_dir.path().join("bin")).unwrap();
        fs::create_dir_all(install_dir.path().join("lib")).unwrap();

        std::env::set_var("SHELL", "/bin/zsh");

        shell_utils::setup_path(install_dir.path()).unwrap();

        let rc_content = fs::read_to_string(&zshenv).unwrap();
        let expected_source = format!(r#". "{}/env""#, install_dir.path().display());
        assert!(rc_content.contains(existing));
        assert!(rc_content.contains(&expected_source));

        shell_utils::uninstall_path(install_dir.path()).unwrap();
        let rc_after = fs::read_to_string(&zshenv).unwrap_or_default();
        assert!(rc_after.contains(existing));
        assert!(!rc_after.contains(&expected_source));
    }
}

#[cfg(all(test, windows))]
mod setup_uninstall_windows {
    use serial_test::serial;
    use wasmedgeup::shell_utils;
    use winreg::enums::*;
    use winreg::RegKey;

    fn open_env_key() -> RegKey {
        let hkcu = RegKey::predef(HKEY_CURRENT_USER);
        hkcu.open_subkey_with_flags("Environment", KEY_READ | KEY_WRITE)
            .expect("failed to open HKCU\\Environment")
    }

    fn get_path_value(env: &RegKey) -> Option<String> {
        match env.get_value::<String, _>("Path") {
            Ok(v) => Some(v),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
            Err(e) => panic!("unexpected registry error: {e}"),
        }
    }

    fn set_path_value(env: &RegKey, val: &str) {
        env.set_value("Path", &val).expect("failed to set Path");
    }

    fn delete_path_value(env: &RegKey) {
        let _ = env.delete_value("Path");
    }

    #[test]
    #[serial]
    fn test_no_rc_value() {
        let env = open_env_key();
        let backup = get_path_value(&env);
        delete_path_value(&env);

        let install_dir = tempfile::tempdir().unwrap();
        let bin_path = format!("{}\\bin", install_dir.path().display());

        shell_utils::setup_path(install_dir.path()).unwrap();

        let now = get_path_value(&env).unwrap_or_default();
        assert_eq!(now, bin_path);

        shell_utils::uninstall_path(install_dir.path()).unwrap();
        shell_utils::uninstall_path(install_dir.path()).unwrap();

        let after = get_path_value(&env);
        assert!(after.is_none() || after.unwrap().is_empty());

        match backup {
            Some(v) => set_path_value(&env, &v),
            None => delete_path_value(&env),
        }
    }

    #[test]
    #[serial]
    fn test_empty_value() {
        let env = open_env_key();
        let backup = get_path_value(&env);
        set_path_value(&env, "");

        let install_dir = tempfile::tempdir().unwrap();
        let bin_path = format!("{}\\bin", install_dir.path().display());

        shell_utils::setup_path(install_dir.path()).unwrap();

        let now = get_path_value(&env).unwrap();
        assert_eq!(now, bin_path);

        shell_utils::uninstall_path(install_dir.path()).unwrap();
        let after = get_path_value(&env).unwrap_or_default();
        assert!(after.is_empty());

        match backup {
            Some(v) => set_path_value(&env, &v),
            None => delete_path_value(&env),
        }
    }

    #[test]
    #[serial]
    fn test_existing_value() {
        let env = open_env_key();
        let backup = get_path_value(&env);
        let existing = r"C:\\Some\\Where";
        set_path_value(&env, existing);

        let install_dir = tempfile::tempdir().unwrap();
        let bin_path = format!("{}\\bin", install_dir.path().display());

        shell_utils::setup_path(install_dir.path()).unwrap();

        let now = get_path_value(&env).unwrap();
        assert!(
            now.ends_with(&bin_path),
            "{now} does not end with {bin_path}"
        );
        assert!(now.starts_with(existing));

        shell_utils::uninstall_path(install_dir.path()).unwrap();
        let after = get_path_value(&env).unwrap();
        assert_eq!(after, existing);

        match backup {
            Some(v) => set_path_value(&env, &v),
            None => delete_path_value(&env),
        }
    }
}

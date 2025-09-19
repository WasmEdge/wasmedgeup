use std::env;
use tempfile::TempDir;
use wasmedgeup::prelude::*;
use wasmedgeup::shell_utils;

mod test_utils;
use test_utils::setup_test_environment;

#[test]
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

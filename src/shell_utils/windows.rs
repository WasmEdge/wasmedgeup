use crate::{
    commands::normalize_absolute_path,
    error::{Result, WindowsRegistrySnafu},
    shell_utils::ManagedShellScriptCleanup,
};
use cap_std::fs::Dir;
use snafu::ResultExt;
use std::path::Path;
use winreg::enums::*;
use winreg::RegKey;

pub fn setup_path(install_dir: &Path) -> Result<()> {
    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    let env = hkcu
        .open_subkey_with_flags("Environment", KEY_READ | KEY_WRITE)
        .context(WindowsRegistrySnafu)?;

    let current_path = match env.get_value("Path") {
        Ok(path) => path,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(e).context(WindowsRegistrySnafu),
    };
    let bin_path = format!("{}\\{}", install_dir.display(), "bin");

    // Normalize paths for comparison and to avoid duplicates with different casing
    // And since we cannot assume that the paths are ASCII strings, we can only use to_lowercase etc.
    let norm_bin_path = bin_path.to_lowercase();
    let already_exists = current_path
        .split(';')
        .any(|p| p.trim().to_lowercase() == norm_bin_path);

    if already_exists {
        return Ok(());
    }

    let new_path = if current_path.is_empty() || current_path.ends_with(';') {
        format!("{}{}", current_path, bin_path)
    } else {
        format!("{};{}", current_path, bin_path)
    };

    env.set_value("Path", &new_path)
        .context(WindowsRegistrySnafu)?;

    Ok(())
}

pub(crate) fn setup_path_in(
    _install_root: &Dir,
    _staging_root: &Dir,
    install_dir: &Path,
) -> Result<()> {
    setup_path(install_dir)
}

pub(crate) fn validate_shell_configuration_path(_install_dir: &Path) -> Result<()> {
    Ok(())
}

pub fn uninstall_path(install_dir: &Path) -> Result<()> {
    uninstall_path_configuration(install_dir)?;
    let target_dir = normalize_absolute_path(install_dir)?;
    if target_dir != install_dir {
        uninstall_path_configuration(&target_dir)?;
    }
    Ok(())
}

pub fn uninstall_path_configuration(install_dir: &Path) -> Result<()> {
    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    let env = hkcu
        .open_subkey_with_flags("Environment", KEY_READ | KEY_WRITE)
        .context(WindowsRegistrySnafu)?;

    let current_path: String = match env.get_value("Path") {
        Ok(path) => path,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(e).context(WindowsRegistrySnafu),
    };

    if current_path.is_empty() {
        return Ok(());
    }

    let bin_path = format!("{}\\{}", install_dir.display(), "bin");
    let norm_bin_path = bin_path.to_lowercase();

    let mut parts: Vec<String> = current_path.split(';').map(|s| s.to_string()).collect();

    let original_len = parts.len();
    parts.retain(|p| p.trim().to_lowercase() != norm_bin_path);

    if parts.len() == original_len {
        return Ok(());
    }

    let new_path = parts
        .into_iter()
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join(";");

    env.set_value("Path", &new_path)
        .context(WindowsRegistrySnafu)?;

    Ok(())
}

pub(crate) fn remove_managed_shell_scripts(
    _install_root: &Dir,
    _target_dir: &Path,
    _configured_target_dir: &Path,
) -> ManagedShellScriptCleanup {
    ManagedShellScriptCleanup {
        configured_paths: Vec::new(),
        result: Ok(()),
    }
}

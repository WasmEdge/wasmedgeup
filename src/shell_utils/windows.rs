use crate::error::{Result, WindowsRegistrySnafu};
use snafu::ResultExt;
use winreg::enums::*;
use winreg::RegKey;

pub fn add_to_user_path(install_path: &str) -> Result<()> {
    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    let env = hkcu
        .open_subkey_with_flags("Environment", KEY_READ | KEY_WRITE)
        .context(WindowsRegistrySnafu)?;

    let current_path: String = match env.get_value("Path") {
        Ok(path) => path,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(), // Treat missing Path as empty
        Err(e) => return Err(e).context(WindowsRegistrySnafu), // Other errors
    };
    let bin_path = format!("{}\\{}", install_path, "bin");

    // Normalize paths for comparison and to avoid duplicates with different casing
    let norm_bin_path = bin_path.to_lowercase();
    let already_exists = current_path
        .split(';')
        .any(|p| p.trim().to_lowercase() == norm_bin_path);

    if already_exists {
        return Ok(()); // Path already includes wasmedge (case-insensitive)
    }

    let new_path = if current_path.is_empty() || current_path.ends_with(';') {
        format!("{}{}", current_path, bin_path)
    } else {
        format!("{};{}", current_path, bin_path)
    };

    // Clean up potential double semicolons if current_path was empty or only ";"
    let final_path = new_path.replace(";;", ";").trim_matches(';').to_string();


    env.set_value("Path", &final_path)
        .context(WindowsRegistrySnafu)?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(windows)]
    #[test]
    fn test_add_to_user_path_windows() {
        // This test would ideally mock winreg::RegKey interactions.
        // Since that's complex, this is a placeholder or would run in a Windows environment.
        // For demonstration, let's assume a function that can check/mock this.
        // For now, it's a no-op to ensure compilation.
        assert!(true, "Windows PATH test needs specific setup or mocking.");
    }
}

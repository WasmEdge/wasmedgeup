use crate::config::Config;
use crate::error::WasmedgeupError;
use clap::ArgMatches;
use anyhow::Result;
use std::fs;
use std::path::Path;
use std::io::{self, Write};

pub async fn execute(matches: &ArgMatches) -> Result<()> {
    let config = Config::load()?;
    let remove_cmd = RemoveCommand::from_matches(matches, &config)?;
    remove_cmd.execute().await
}

pub struct RemoveCommand {
    version: Option<String>,
    remove_all: bool,
    install_path: String,
}

impl RemoveCommand {
    pub fn from_matches(matches: &ArgMatches, config: &Config) -> Result<Self> {
        Ok(Self {
            version: matches.get_one::<String>("version").cloned(),
            remove_all: matches.get_flag("all"),
            install_path: config.install_path.clone(),
        })
    }

    pub async fn execute(&self) -> Result<()> {
        if self.remove_all {
            self.remove_all_versions().await
        } else {
            let version = match &self.version {
                Some(v) => v.clone(),
                None => self.get_current_version()?,
            };
            self.remove_version(&version).await
        }
    }

    async fn remove_version(&self, version: &str) -> Result<()> {
        println!("Removing WasmEdge version: {}", version);
        
        let install_dir = Path::new(&self.install_path);
        
        // Remove version-specific directory
        let version_dir = install_dir.join("versions").join(version);
        if version_dir.exists() {
            fs::remove_dir_all(&version_dir)?;
            println!("✓ Removed version directory: {}", version);
        } else {
            return Err(WasmedgeupError::VersionNotFound(version.to_string()).into());
        }

        // Remove symlinks if this was the active version
        let bin_dir = install_dir.join("bin");
        if bin_dir.exists() && self.is_active_version(version)? {
            fs::remove_dir_all(&bin_dir)?;
            println!("✓ Removed binary symlinks");
        }

        // Update version registry
        self.update_version_registry(version, false)?;
        
        println!("✓ Successfully removed WasmEdge {}", version);
        Ok(())
    }

    async fn remove_all_versions(&self) -> Result<()> {
        print!("This will remove ALL installed WasmEdge versions. Continue? (y/N): ");
        io::stdout().flush()?;
        
        let mut input = String::new();
        io::stdin().read_line(&mut input)?;
        
        if !input.trim().to_lowercase().starts_with('y') {
            println!("Cancelled.");
            return Ok(());
        }

        let install_dir = Path::new(&self.install_path);
        
        // Remove entire .wasmedge directory
        if install_dir.exists() {
            fs::remove_dir_all(install_dir)?;
            println!("✓ Removed all WasmEdge installations");
        }

        // Remove from PATH (platform-specific)
        self.remove_from_path()?;
        
        println!("✓ Successfully removed all WasmEdge versions");
        Ok(())
    }

    fn get_current_version(&self) -> Result<String> {
        let current_file = Path::new(&self.install_path).join("current");
        
        if current_file.exists() {
            let version = fs::read_to_string(current_file)?;
            Ok(version.trim().to_string())
        } else {
            // Try to find the latest installed version
            self.get_latest_installed_version()
        }
    }

    fn get_latest_installed_version(&self) -> Result<String> {
        let versions_dir = Path::new(&self.install_path).join("versions");
        
        if !versions_dir.exists() {
            return Err(WasmedgeupError::NoVersionsInstalled.into());
        }

        let mut versions = Vec::new();
        for entry in fs::read_dir(&versions_dir)? {
            let entry = entry?;
            if entry.file_type()?.is_dir() {
                if let Some(name) = entry.file_name().to_str() {
                    versions.push(name.to_string());
                }
            }
        }

        if versions.is_empty() {
            return Err(WasmedgeupError::NoVersionsInstalled.into());
        }

        // Sort versions and return the latest
        versions.sort();
        Ok(versions.last().unwrap().clone())
    }

    fn is_active_version(&self, version: &str) -> Result<bool> {
        match self.get_current_version() {
            Ok(current) => Ok(current == version),
            Err(_) => Ok(false),
        }
    }

    fn update_version_registry(&self, version: &str, add: bool) -> Result<()> {
        let registry_file = Path::new(&self.install_path).join("versions.json");
        
        let mut versions: Vec<String> = if registry_file.exists() {
            let content = fs::read_to_string(&registry_file)?;
            serde_json::from_str(&content).unwrap_or_default()
        } else {
            Vec::new()
        };

        if add {
            if !versions.contains(&version.to_string()) {
                versions.push(version.to_string());
            }
        } else {
            versions.retain(|v| v != version);
        }

        let content = serde_json::to_string_pretty(&versions)?;
        fs::write(registry_file, content)?;
        
        Ok(())
    }

    fn remove_from_path(&self) -> Result<()> {
        // Platform-specific PATH removal logic
        #[cfg(unix)]
        {
            // For Unix systems, we could update shell profiles
            // This is a simplified version - real implementation would be more robust
            println!("Note: Please remove {} from your PATH manually", 
                     Path::new(&self.install_path).join("bin").display());
        }
        
        #[cfg(windows)]
        {
            // For Windows, we could update registry
            println!("Note: Please remove {} from your PATH manually", 
                     Path::new(&self.install_path).join("bin").display());
        }
        
        Ok(())
    }
}

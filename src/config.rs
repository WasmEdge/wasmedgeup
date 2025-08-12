use crate::error::WasmedgeupError;
use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::Path;

#[derive(Debug, Deserialize, Serialize)]
pub struct Config {
    pub install_path: String,
    pub temp_dir: String,
}

impl Config {
    pub fn load() -> Result<Self> {
        let home_dir = dirs::home_dir()
            .ok_or_else(|| WasmedgeupError::IoError(
                std::io::Error::new(std::io::ErrorKind::NotFound, "Home directory not found")
            ))?;
        
        let install_path = home_dir.join(".wasmedge").to_string_lossy().to_string();
        let temp_dir = std::env::temp_dir().to_string_lossy().to_string();
        
        Ok(Self {
            install_path,
            temp_dir,
        })
    }
    
    pub fn get_current_version(&self) -> Result<String> {
        let current_file = Path::new(&self.install_path).join("current");
        
        if current_file.exists() {
            let version = fs::read_to_string(current_file)?;
            Ok(version.trim().to_string())
        } else {
            Err(WasmedgeupError::NoVersionsInstalled.into())
        }
    }
}
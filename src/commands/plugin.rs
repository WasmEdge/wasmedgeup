use crate::config::Config;
use crate::error::WasmedgeupError;
use crate::platform::PlatformInfo;
use clap::ArgMatches;
use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::Path;

pub async fn execute(matches: &ArgMatches) -> Result<()> {
    let config = Config::load()?;
    
    match matches.subcommand() {
        Some(("list", sub_matches)) => {
            list_plugins(sub_matches, &config).await
        }
        Some(("install", sub_matches)) => {
            install_plugins(sub_matches, &config).await
        }
        Some(("remove", sub_matches)) => {
            remove_plugins(sub_matches, &config).await
        }
        _ => {
            println!("Use 'wasmedgeup plugin --help' for usage information");
            Ok(())
        }
    }
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct PluginManifest {
    pub maintained: Vec<String>,
    pub deprecated: Vec<String>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct PluginInfo {
    pub deps: Vec<String>,
    pub platform: Vec<String>,
}

pub struct PluginManager {
    wasmedge_version: String,
    install_path: String,
    platform: PlatformInfo,
}

impl PluginManager {
    pub fn new(config: &Config) -> Result<Self> {
        let wasmedge_version = config.get_current_version()
            .unwrap_or_else(|_| "latest".to_string());
        
        Ok(Self {
            wasmedge_version,
            install_path: config.install_path.clone(),
            platform: PlatformInfo::detect(),
        })
    }

    pub async fn list_available_plugins(&self) -> Result<HashMap<String, Vec<String>>> {
        let mut all_plugins = HashMap::new();
        
        // Fetch from cpp_plugins repository
        if let Ok(cpp_plugins) = self.fetch_plugin_manifest(
            "https://github.com/WasmEdge/cpp_plugins/releases/latest/download/version.json"
        ).await {
            for plugin in cpp_plugins.maintained {
                all_plugins.entry(plugin.clone())
                    .or_insert_with(Vec::new)
                    .push("cpp".to_string());
            }
        }

        // Fetch from rust_plugins repository
        if let Ok(rust_plugins) = self.fetch_plugin_manifest(
            "https://github.com/WasmEdge/rust_plugins/releases/latest/download/version.json"
        ).await {
            for plugin in rust_plugins.maintained {
                all_plugins.entry(plugin.clone())
                    .or_insert_with(Vec::new)
                    .push("rust".to_string());
            }
        }

        Ok(all_plugins)
    }

    pub fn list_installed_plugins(&self) -> Result<Vec<String>> {
        let plugin_dir = Path::new(&self.install_path).join("plugins");
        let mut installed = Vec::new();

        if plugin_dir.exists() {
            for entry in fs::read_dir(&plugin_dir)? {
                let entry = entry?;
                if entry.file_type()?.is_dir() {
                    if let Some(name) = entry.file_name().to_str() {
                        installed.push(name.to_string());
                    }
                }
            }
        }

        installed.sort();
        Ok(installed)
    }

    pub async fn install_plugin(&self, plugin_name: &str, version: Option<&str>) -> Result<()> {
        println!("Installing plugin: {}", plugin_name);
        
        // Create plugin directory
        let plugin_dir = Path::new(&self.install_path)
            .join("plugins")
            .join(plugin_name);
        fs::create_dir_all(&plugin_dir)?;

        // Determine plugin type and download URL
        let plugin_url = self.construct_plugin_url(plugin_name, version).await?;
        
        // Download plugin
        let response = reqwest::get(&plugin_url).await?;
        if !response.status().is_success() {
            return Err(WasmedgeupError::PluginNotFound(plugin_name.to_string()).into());
        }

        // Save plugin file
        let plugin_file = plugin_dir.join(format!("lib{}.so", plugin_name));
        let bytes = response.bytes().await?;
        fs::write(&plugin_file, bytes)?;

        println!("✓ Successfully installed plugin: {}", plugin_name);
        Ok(())
    }

    pub async fn remove_plugin(&self, plugin_name: &str) -> Result<()> {
        println!("Removing plugin: {}", plugin_name);
        
        let plugin_dir = Path::new(&self.install_path)
            .join("plugins")
            .join(plugin_name);
            
        if plugin_dir.exists() {
            fs::remove_dir_all(&plugin_dir)?;
            println!("✓ Successfully removed plugin: {}", plugin_name);
        } else {
            println!("Plugin {} not found", plugin_name);
        }
        
        Ok(())
    }

    async fn fetch_plugin_manifest(&self, url: &str) -> Result<PluginManifest> {
        let response = reqwest::get(url).await?;
        let manifest: PluginManifest = response.json().await?;
        Ok(manifest)
    }

    async fn construct_plugin_url(&self, plugin_name: &str, version: Option<&str>) -> Result<String> {
        let version = version.unwrap_or("latest");
        let platform_str = self.platform.to_plugin_platform_string();
        
        // Try cpp plugins first
        let cpp_url = format!(
            "https://github.com/WasmEdge/cpp_plugins/releases/download/{}/lib{}-{}.so",
            version, plugin_name, platform_str
        );
        
        // Check if cpp plugin exists
        let response = reqwest::head(&cpp_url).await?;
        if response.status().is_success() {
            return Ok(cpp_url);
        }

        // Try rust plugins
        let rust_url = format!(
            "https://github.com/WasmEdge/rust_plugins/releases/download/{}/lib{}-{}.so",
            version, plugin_name, platform_str
        );
        
        Ok(rust_url)
    }
}

async fn list_plugins(matches: &ArgMatches, config: &Config) -> Result<()> {
    let manager = PluginManager::new(config)?;
    let installed_only = matches.get_flag("installed");

    if installed_only {
        let installed = manager.list_installed_plugins()?;
        if installed.is_empty() {
            println!("No plugins installed");
        } else {
            println!("Installed plugins:");
            for plugin in installed {
                println!("  {}", plugin);
            }
        }
    } else {
        println!("Fetching available plugins...");
        let available = manager.list_available_plugins().await?;
        let installed = manager.list_installed_plugins().unwrap_or_default();

        if available.is_empty() {
            println!("No plugins available");
            return Ok(());
        }

        println!("Available plugins:");
        for (plugin, types) in available {
            let status = if installed.contains(&plugin) {
                " (installed)"
            } else {
                ""
            };
            println!("  {} [{}]{}", plugin, types.join(", "), status);
        }
    }

    Ok(())
}

async fn install_plugins(matches: &ArgMatches, config: &Config) -> Result<()> {
    let manager = PluginManager::new(config)?;
    let plugins: Vec<&String> = matches.get_many::<String>("plugins").unwrap().collect();
    let version = matches.get_one::<String>("version");

    for plugin in plugins {
        if let Err(e) = manager.install_plugin(plugin, version.map(|s| s.as_str())).await {
            eprintln!("Failed to install {}: {}", plugin, e);
        }
    }

    Ok(())
}

async fn remove_plugins(matches: &ArgMatches, config: &Config) -> Result<()> {
    let manager = PluginManager::new(config)?;
    let plugins: Vec<&String> = matches.get_many::<String>("plugins").unwrap().collect();

    for plugin in plugins {
        if let Err(e) = manager.remove_plugin(plugin).await {
            eprintln!("Failed to remove {}: {}", plugin, e);
        }
    }

    Ok(())
}
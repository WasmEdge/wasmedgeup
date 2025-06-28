use std::path::PathBuf;

use clap::Parser;
use semver::Version;
use snafu::ResultExt;
use tokio::fs;

use crate::{
    api::{Asset, WasmEdgeApiClient},
    cli::{CommandContext, CommandExecutor},
    error::IoSnafu, // Import IoSnafu
    prelude::*,
    shell_utils, // Import shell_utils
    target::{TargetArch, TargetOS},
};

fn default_path() -> PathBuf {
    let home_dir = dirs::home_dir().expect("home_dir should be present");
    home_dir.join(".wasmedge")
}

fn default_tmpdir() -> PathBuf {
    std::env::temp_dir()
}

#[derive(Debug, Parser)]
pub struct InstallArgs {
    /// WasmEdge version to install, e.g. `latest`, `0.14.1`, `0.14.1-rc.1`, etc.
    pub version: String,

    /// Set the install location for the WasmEdge runtime
    ///
    /// Defaults to `$HOME/.wasmedge` on Unix-like systems and `%HOME%\.wasmedge` on Windows.
    #[arg(short, long)]
    pub path: Option<PathBuf>,

    /// Set the temporary directory for staging downloaded assets
    ///
    /// Defaults to the system temporary directory, this differs between operating systems.
    #[arg(short, long)]
    pub tmpdir: Option<PathBuf>,

    /// Set the target OS for the WasmEdge runtime
    ///
    /// `wasmedgeup` will detect the OS of your host system by default.
    #[arg(short, long)]
    pub os: Option<TargetOS>,

    /// Set the target architecture for the WasmEdge runtime
    ///
    /// `wasmedgeup` will detect the architecture of your host system by default.
    #[arg(short, long)]
    pub arch: Option<TargetArch>,
}

impl CommandExecutor for InstallArgs {
    /// Executes the installation process by resolving the version, downloading the asset,
    /// unpacking it, and copying the extracted files to the target directory.
    ///
    /// # Steps:
    /// 1. Resolves the version (either a specific version or the latest).
    /// 2. Downloads the asset for the appropriate OS and architecture.
    /// 3. Unpacks the asset to a temporary directory.
    /// 4. Copies the extracted files to the target directory.
    ///
    /// # Arguments
    ///
    /// * `ctx` - The command context containing the client and progress bar settings.
    ///
    /// # Errors
    ///
    /// Returns an error if any step fails, such as download failure, extraction issues,
    /// or copying issues.
    #[tracing::instrument(name = "install", skip_all, fields(version = self.version))]
    async fn execute(mut self, ctx: CommandContext) -> Result<()> {
        let version = self.resolve_version(&ctx.client).inspect_err(
            |e| tracing::error!(error = %e.to_string(), "Failed to resolve version"),
        )?;
        tracing::debug!(%version, "Resolved version for installation");

        let os = self.os.get_or_insert_default();
        let arch = self.arch.get_or_insert_default();
        tracing::debug!(?os, ?arch, "Host OS and architecture detected");

        let asset = Asset::new(&version, os, arch);
        let tmpdir = self.tmpdir.unwrap_or_else(default_tmpdir);
        let file = ctx
            .client
            .download_asset(&asset, &tmpdir, ctx.no_progress)
            .await
            .inspect_err(|e| tracing::error!(error = %e.to_string(), "Failed to download asset"))?;

        tracing::debug!(file_path = %file.path().display(), dest = %tmpdir.display(), "Starting extraction of asset");
        crate::fs::extract_archive(file.into_file(), &tmpdir)
            .await
            .inspect_err(|e| tracing::error!(error = %e.to_string(), "Failed to extract asset"))?;
        tracing::debug!(dest = %tmpdir.display(), "Extraction completed successfully");

        // Try with `tmpdir/WasmEdge-{version}-{os}` first, and if it's not a directory, fallback
        // to `tmpdir`
        // (both patterns are used in WasmEdge)
        let mut extracted_dir = tmpdir.join(&asset.install_name);
        if !extracted_dir.is_dir() {
            tracing::debug!(extracted_dir = %extracted_dir.display(), "Falling back to tmpdir as extracted directory");
            extracted_dir = tmpdir;
        }

        let target_dir = self.path.unwrap_or_else(default_path);
        tracing::debug!(extracted_dir = %extracted_dir.display(), target_dir = %target_dir.display(), "Start copying files to target location");
        crate::fs::copy_tree(&extracted_dir, &target_dir).await;
        tracing::debug!(target_dir = %target_dir.display(), "Copying files to target location completed");

        // Create the environment file
        let env_file_path = target_dir.join("env");
        let env_content = format!(
            "export PATH=\"{}/bin:$PATH\"",
            target_dir.to_string_lossy()
        );
        // Remove the old generic env file creation
        // fs::write(&env_file_path, env_content)
        //     .await
        //     .context(IoSnafu)?;
        // tracing::debug!(env_file_path = %env_file_path.display(), "Environment file created successfully");

        // Add to PATH
        if cfg!(windows) {
            // On Windows, target_dir is %USERPROFILE%\.wasmedge
            shell_utils::setup_path(&target_dir)?;
            println!("WasmEdge added to User PATH. Please restart your shell or log off and on for changes to take effect.");
        } else {
            let home_dir = dirs::home_dir().ok_or(crate::error::Error::HomeDirNotFound)?;
            let wasmedge_dot_dir = target_dir; // This is $HOME/.wasmedge or custom path

            // Ensure .wasmedge directory exists (it should, as we just installed to it)
            fs::create_dir_all(&wasmedge_dot_dir).await.context(IoSnafu)?;

            // Ensure .wasmedge directory exists (it should, as we just installed to it)
            fs::create_dir_all(&wasmedge_dot_dir).await.context(IoSnafu)?;

            let wasmedge_bin_dir_str = wasmedge_dot_dir.join("bin").to_string_lossy().to_string();
            let shells = shell_utils::unix::get_supported_shells();
            let mut shells_updated_count = 0;

            for shell_handler in shells {
                if shell_handler.is_present(&home_dir) {
                    let script_details = shell_handler.env_script();
                    let env_script_name = script_details.name; // e.g., "env.sh"
                    let env_script_path = wasmedge_dot_dir.join(env_script_name);

                    // Write the shell-specific env script
                    let script_content = script_details.template
                        .replace("{WASMEDGE_BIN_DIR}", &wasmedge_bin_dir_str);
                    fs::write(&env_script_path, &script_content).await.context(IoSnafu)?;
                    tracing::debug!(env_file_path = %env_script_path.display(), "{} script written successfully", env_script_name);

                    let source_line = shell_handler.source_line(&env_script_path);

                    if let Some(rc_file_path) = shell_handler.effective_rc_file(&home_dir) {
                        // append_to_file_if_not_present will create the file if it doesn't exist.
                        // For Nushell, effective_rc_file already ensures it found an existing file,
                        // so this is fine. For others, it provides the standard path.
                        // Zsh targets .zshenv, which is fine to create if not present.
                        shell_utils::unix::append_to_file_if_not_present(&rc_file_path, &source_line).await?;
                        println!(
                            "Updated {} to source WasmEdge {} env. Please restart your shell or source {}.",
                            rc_file_path.display(),
                            shell_handler.name(),
                            rc_file_path.display() // Suggests sourcing the rc file itself
                        );
                        shells_updated_count += 1;
                    } else {
                        // This case would primarily be for Nushell if no config file was found.
                        println!(
                            "Could not find an existing configuration file for {}. WasmEdge env script is at {}. Please source it manually.",
                            shell_handler.name(),
                            env_script_path.display()
                        );
                    }
                }
            }

            if shells_updated_count > 0 {
                println!(
                    "WasmEdge environment scripts are available in {}. Please source the appropriate one if your shell was not automatically configured.",
                    wasmedge_dot_dir.display()
                );
            } else {
                println!(
                    "No supported shell configuration files found or updated. WasmEdge environment scripts are available at {}. Please source the appropriate one manually.",
                    wasmedge_dot_dir.display()
                );
            }
        }

        Ok(())
    }
}

impl InstallArgs {
    fn resolve_version(&self, client: &WasmEdgeApiClient) -> Result<Version> {
        if self.version == "latest" {
            client.latest_release()
        } else {
            Version::parse(&self.version).context(SemVerSnafu {})
        }
    }
}

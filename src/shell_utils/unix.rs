use crate::prelude::*;

use dirs::home_dir;
use snafu::OptionExt;
use std::io::Write;
use std::path::{Path, PathBuf};

pub fn setup_path(install_dir: &Path) -> Result<()> {
    use std::fs::read_to_string;

    let mut written = vec![];

    for shell in get_available_shells() {
        let env_script = shell.env_script();

        // Write each script only once
        if !written.contains(&env_script) {
            shell.write_script(&env_script, install_dir)?;
            written.push(env_script);
        }

        let script_path = install_dir.join(env_script.name);
        let source_line = shell.source_line(&script_path);
        let source_line_with_newline = format!("\n{}", &source_line);

        for rc in shell.effective_rc_files() {
            let line_to_write: &str = match read_to_string(&rc) {
                Ok(content) if content.contains(&source_line) => continue,
                Ok(content) if !content.ends_with('\n') => &source_line_with_newline,
                _ => &source_line,
            };

            let rc_dir = rc.parent().context(RcDirNotFoundSnafu {
                path: rc.display().to_string(),
            })?;
            if !rc_dir.is_dir() {
                std::fs::create_dir_all(rc_dir)?;
            }

            append_file(&rc, line_to_write)?;
        }
    }

    Ok(())
}

pub fn get_supported_shells() -> Vec<Shell> {
    vec![
        Box::new(Posix),
        Box::new(Bash),
        Box::new(Zsh),
        Box::new(Fish),
        Box::new(Nushell),
    ]
}

pub fn get_available_shells() -> Vec<Shell> {
    get_supported_shells()
        .into_iter()
        .filter(|shell| shell.is_present())
        .collect()
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ShellScript {
    pub template: &'static str,
    pub name: &'static str,
}

pub trait UnixShell: Send + Sync {
    fn is_present(&self) -> bool;

    fn potential_rc_paths(&self) -> Vec<PathBuf>;
    fn effective_rc_files(&self) -> Vec<PathBuf>;

    fn env_script(&self) -> ShellScript {
        ShellScript {
            name: "env",
            template: include_str!("env.sh"),
        }
    }

    fn source_line(&self, install_dir: &Path) -> String {
        format!(r#". "{}/env""#, install_dir.to_string_lossy())
    }

    fn write_script(&self, script: &ShellScript, install_dir: &Path) -> Result<()> {
        let wasmedge_bin = format!("{}/bin", install_dir.to_string_lossy());
        let env_path = install_dir.join(script.name);
        let env_content = script.template.replace("{WASMEDGE_BIN_DIR}", &wasmedge_bin);

        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(env_path)?;

        file.write_all(env_content.as_bytes())?;
        file.sync_data()?;

        Ok(())
    }
}

pub type Shell = Box<dyn UnixShell>;

#[derive(Debug, Default)]
pub struct Posix;
impl UnixShell for Posix {
    fn is_present(&self) -> bool {
        true
    }

    fn potential_rc_paths(&self) -> Vec<PathBuf> {
        home_dir()
            .into_iter()
            .map(|dir| dir.join(".profile"))
            .collect()
    }

    fn effective_rc_files(&self) -> Vec<PathBuf> {
        self.potential_rc_paths()
    }
}

#[derive(Debug, Default)]
pub struct Bash;

impl UnixShell for Bash {
    fn is_present(&self) -> bool {
        !self.effective_rc_files().is_empty()
    }

    fn potential_rc_paths(&self) -> Vec<PathBuf> {
        [".bash_profile", ".bash_login", ".bashrc"]
            .iter()
            .filter_map(|name| home_dir().map(|dir| dir.join(name)))
            .collect()
    }

    fn effective_rc_files(&self) -> Vec<PathBuf> {
        self.potential_rc_paths()
            .into_iter()
            .filter(|rc| rc.is_file())
            .collect()
    }
}

// Zsh Implementation
#[derive(Debug, Default)]
pub struct Zsh;

impl Zsh {
    fn zdotdir() -> Result<PathBuf> {
        use std::ffi::OsStr;
        use std::os::unix::ffi::OsStrExt;

        if matches!(std::env::var("SHELL"), Ok(sh) if sh.ends_with("/zsh")) {
            return match std::env::var("ZDOTDIR") {
                Ok(dir) if !dir.is_empty() => Ok(PathBuf::from(dir)),
                _ => Err(Error::Unknown),
            };
        }

        match std::process::Command::new("zsh")
            .args(["-c", "echo -n $ZDOTDIR"])
            .output()
        {
            Ok(io) if !io.stdout.is_empty() => Ok(PathBuf::from(OsStr::from_bytes(&io.stdout))),
            _ => Err(Error::Unknown),
        }
    }
}

impl UnixShell for Zsh {
    fn is_present(&self) -> bool {
        matches!(std::env::var("SHELL"), Ok(sh) if sh.ends_with("/zsh"))
            || is_command_in_path("zsh")
    }

    fn potential_rc_paths(&self) -> Vec<PathBuf> {
        [Zsh::zdotdir().ok(), home_dir()]
            .iter()
            .filter_map(|dir| dir.as_ref().map(|p| p.join(".zshenv")))
            .collect()
    }

    fn effective_rc_files(&self) -> Vec<PathBuf> {
        self.potential_rc_paths()
            .into_iter()
            .filter(|rc| rc.is_file())
            .chain(self.potential_rc_paths())
            .take(1)
            .collect()
    }
}

// Fish Implementation
#[derive(Debug, Default)]
pub struct Fish;
impl UnixShell for Fish {
    fn is_present(&self) -> bool {
        matches!(std::env::var("SHELL"), Ok(sh) if sh.ends_with("/fish"))
            || is_command_in_path("fish")
    }

    // > "$XDG_CONFIG_HOME/fish/conf.d" (or "~/.config/fish/conf.d" if that variable is unset) for the user
    // from <https://github.com/fish-shell/fish-shell/issues/3170#issuecomment-228311857>
    fn potential_rc_paths(&self) -> Vec<PathBuf> {
        let xdg_rc_path = std::env::var("XDG_CONFIG_HOME").ok().map(|p| {
            let mut p = PathBuf::from(p);
            p.extend(["fish", "conf.d", "wasmedgeup.fish"]);
            p
        });

        let home_rc_path = home_dir().map(|mut p| {
            p.extend([".config", "fish", "conf.d", "wasmedgeup.fish"]);
            p
        });

        xdg_rc_path.into_iter().chain(home_rc_path).collect()
    }

    fn effective_rc_files(&self) -> Vec<PathBuf> {
        // The take first one
        self.potential_rc_paths()
            .into_iter()
            .next()
            .into_iter()
            .collect()
    }

    fn env_script(&self) -> ShellScript {
        ShellScript {
            template: include_str!("env.fish"),
            name: "env.fish",
        }
    }

    fn source_line(&self, install_dir: &Path) -> String {
        format!(r#"source "{}/env.fish"#, install_dir.to_string_lossy())
    }
}

// Nushell Implementation
#[derive(Debug, Default)]
pub struct Nushell;
impl UnixShell for Nushell {
    fn is_present(&self) -> bool {
        matches!(std::env::var("SHELL"), Ok(sh) if sh.ends_with("/nu")) || is_command_in_path("nu")
    }

    fn potential_rc_paths(&self) -> Vec<PathBuf> {
        let xdg_rc_path = std::env::var("XDG_CONFIG_HOME").ok().map(|p| {
            let mut p = PathBuf::from(p);
            p.extend(["nushell", "config.nu"]);
            p
        });

        let home_rc_path = home_dir().map(|mut p| {
            p.extend([".config", "nushell", "config.nu"]);
            p
        });

        xdg_rc_path.into_iter().chain(home_rc_path).collect()
    }

    fn effective_rc_files(&self) -> Vec<PathBuf> {
        // The take first one
        self.potential_rc_paths()
            .into_iter()
            .next()
            .into_iter()
            .collect()
    }

    fn env_script(&self) -> ShellScript {
        ShellScript {
            template: include_str!("env.nu"),
            name: "env.nu",
        }
    }

    fn source_line(&self, install_dir: &Path) -> String {
        format!(r#"source $"{}/env.nu""#, install_dir.to_string_lossy())
    }
}

fn is_command_in_path(command_name: &str) -> bool {
    let Ok(path) = std::env::var("PATH") else {
        return false;
    };

    std::env::split_paths(&path)
        .map(|mut p| {
            p.push(command_name);
            p
        })
        .any(|p| p.is_file())
}

fn append_file(path: &Path, line: &str) -> Result<()> {
    let mut file = std::fs::OpenOptions::new()
        .append(true)
        .create(true)
        .open(path)?;

    writeln!(file, "{}", line)?;

    file.sync_data()?;

    Ok(())
}

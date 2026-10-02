use super::ManagedShellScriptCleanup;
use crate::{
    commands::normalize_absolute_path,
    fs::{
        open_dir_nofollow, quarantine_entry, resolve_trusted_root_alias, restore_quarantined_entry,
    },
    prelude::*,
};

use cap_primitives::fs::FollowSymlinks;
use cap_std::fs::{Dir, OpenOptions as CapOpenOptions};
use dirs::home_dir;
use snafu::OptionExt;
use std::fs::{read_to_string, OpenOptions};
use std::io::{ErrorKind, Read, Write};
use std::path::{Path, PathBuf};

const MAX_MANAGED_SHELL_SCRIPT_BYTES: u64 = 64 * 1024;
const UNICODE_REPLACEMENT_CHARACTER: char = '\u{fffd}';

/// Get XDG config path with fallback to ~/.config.
///
/// Returns the path constructed by joining $XDG_CONFIG_HOME (or ~/.config if unset)
/// with the provided subpath components.
fn xdg_config_path(subpath: &[&str]) -> Option<PathBuf> {
    let base = std::env::var("XDG_CONFIG_HOME")
        .ok()
        .filter(|s| !s.is_empty())
        .map(PathBuf::from)
        .or_else(|| home_dir().map(|h| h.join(".config")))?;

    let mut path = base;
    for component in subpath {
        path.push(component);
    }
    Some(path)
}

pub fn setup_path(install_dir: &Path) -> Result<()> {
    setup_path_impl(None, install_dir)
}

pub(crate) fn setup_path_in(
    install_root: &Dir,
    staging_root: &Dir,
    install_dir: &Path,
) -> Result<()> {
    setup_path_impl(Some((install_root, staging_root)), install_dir)
}

fn setup_path_impl(install_roots: Option<(&Dir, &Dir)>, install_dir: &Path) -> Result<()> {
    use std::fs::read_to_string;

    validate_shell_configuration_path(install_dir)?;

    let mut written = vec![];

    for shell in get_available_shells() {
        let env_script = shell.env_script();

        // Write each script only once
        if !written.contains(&env_script) {
            if let Some((install_root, staging_root)) = install_roots {
                shell.write_script_in(install_root, staging_root, &env_script, install_dir)?;
            } else {
                shell.write_script(&env_script, install_dir)?;
            }
            written.push(env_script);
        }
        let source_line = shell.source_line(install_dir);
        let source_line_with_newline = format!("\n{}", source_line);

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

pub fn uninstall_path(install_dir: &Path) -> Result<()> {
    let target_dir = normalize_absolute_path(install_dir)?;
    let mut configuration_paths = vec![install_dir.to_path_buf()];
    let mut shell_cleanup = Ok(());
    if target_dir != install_dir {
        configuration_paths.push(target_dir.clone());
    }

    match open_dir_nofollow(&target_dir) {
        Ok(install_root) => {
            let cleanup = remove_managed_shell_scripts(&install_root, &target_dir, install_dir);
            for path in cleanup.configured_paths {
                if !configuration_paths.contains(&path) {
                    configuration_paths.push(path);
                }
            }
            shell_cleanup = cleanup.result;
        }
        Err(error) if error.kind() == ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }

    for path in configuration_paths {
        uninstall_path_configuration(&path)?;
    }

    shell_cleanup
}

pub fn uninstall_path_configuration(install_dir: &Path) -> Result<()> {
    // Older releases rendered non-UTF-8 paths with U+FFFD. Multiple distinct
    // byte paths can therefore produce the same source line, so preserving the
    // line is safer than deleting configuration that may belong to another root.
    if shell_path_is_ambiguous(install_dir) {
        tracing::debug!(path = %install_dir.display(), "Preserving ambiguous legacy shell configuration");
        return Ok(());
    }

    for shell in get_supported_shells() {
        let source_line = shell.source_line(install_dir);
        for rc in shell.effective_rc_files() {
            if !rc.exists() {
                continue;
            }

            let Ok(original) = read_to_string(&rc) else {
                continue;
            };
            if let Some(out) = remove_appended_shell_stanza(&original, &source_line) {
                if let Ok(mut file) = OpenOptions::new().write(true).truncate(true).open(&rc) {
                    if let Err(e) = file.write_all(out.as_bytes()) {
                        tracing::warn!(error = %e, path = %rc.display(), "Failed to update shell rc file");
                    }
                    if let Err(e) = file.sync_data() {
                        tracing::warn!(error = %e, path = %rc.display(), "Failed to sync shell rc file");
                    }
                }
            }
        }
    }

    Ok(())
}

fn remove_appended_shell_stanza(original: &str, stanza: &str) -> Option<String> {
    if stanza.is_empty() {
        return None;
    }

    let mut output = String::with_capacity(original.len());
    let mut copied_through = 0;
    let mut search_from = 0;
    let mut changed = false;

    while let Some(relative_start) = original[search_from..].find(stanza) {
        let start = search_from + relative_start;
        let end = start + stanza.len();
        let starts_at_line_boundary = start == 0 || original.as_bytes()[start - 1] == b'\n';
        let trailing_line_ending = if end == original.len() {
            0
        } else if original[end..].starts_with("\r\n") {
            2
        } else if original.as_bytes()[end] == b'\n' {
            1
        } else {
            0
        };
        let ends_at_line_boundary = end == original.len() || trailing_line_ending != 0;

        if starts_at_line_boundary && ends_at_line_boundary {
            output.push_str(&original[copied_through..start]);
            copied_through = end + trailing_line_ending;
            search_from = copied_through;
            changed = true;
        } else {
            let first_character = original[start..]
                .chars()
                .next()
                .expect("find returned a valid character boundary");
            search_from = start + first_character.len_utf8();
        }
    }

    if !changed {
        return None;
    }
    output.push_str(&original[copied_through..]);
    Some(output)
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

impl ShellScript {
    fn content(self, install_dir: &Path) -> String {
        let wasmedge_bin = format!("{}/bin", install_dir.to_string_lossy());
        let wasmedge_lib = format!("{}/{}", install_dir.to_string_lossy(), LIB_DIR);
        let wasmedge_plugin = format!("{}/plugin", install_dir.to_string_lossy());

        self.template
            .replace("{WASMEDGE_BIN_DIR}", &wasmedge_bin)
            .replace("{WASMEDGE_LIB_DIR}", &wasmedge_lib)
            .replace("{WASMEDGE_PLUGIN_DIR}", &wasmedge_plugin)
    }
}

fn shell_path_is_ambiguous(path: &Path) -> bool {
    path.to_string_lossy()
        .contains(UNICODE_REPLACEMENT_CHARACTER)
}

fn shell_path_has_unsafe_syntax(path: &Path) -> bool {
    // `:` cannot represent a literal character in a POSIX PATH entry, and
    // parentheses delimit expressions in Nushell's interpolated source line.
    // The remaining characters either terminate a quoted string, introduce an
    // expansion, or cannot be rendered consistently by every generated script.
    path.to_string_lossy().chars().any(|character| {
        character.is_control() || matches!(character, ':' | '"' | '\\' | '$' | '`' | '(' | ')')
    })
}

pub(crate) fn validate_shell_configuration_path(install_dir: &Path) -> Result<()> {
    if shell_path_is_ambiguous(install_dir) {
        return Err(Error::InvalidPath {
            path: format!("{install_dir:?}"),
            reason: "the install path cannot be represented unambiguously in shell configuration"
                .to_string(),
        });
    }
    if shell_path_has_unsafe_syntax(install_dir) {
        return Err(Error::InvalidPath {
            path: format!("{install_dir:?}"),
            reason: "the install path contains characters that cannot be represented safely in shell configuration"
                .to_string(),
        });
    }
    Ok(())
}

#[cfg(test)]
pub(crate) fn managed_shell_script_content(name: &str, install_dir: &Path) -> Option<String> {
    managed_shell_script(name).map(|script| script.content(install_dir))
}

pub(crate) fn managed_shell_script_install_path(name: &str, content: &str) -> Option<PathBuf> {
    const BIN_PLACEHOLDER: &str = "{WASMEDGE_BIN_DIR}";
    const PLACEHOLDERS: [&str; 3] = [
        BIN_PLACEHOLDER,
        "{WASMEDGE_LIB_DIR}",
        "{WASMEDGE_PLUGIN_DIR}",
    ];

    let script = managed_shell_script(name)?;
    let (prefix, remainder) = script.template.split_once(BIN_PLACEHOLDER)?;
    let next_placeholder = PLACEHOLDERS
        .iter()
        .filter_map(|placeholder| remainder.find(placeholder))
        .min()
        .unwrap_or(remainder.len());
    let suffix = &remainder[..next_placeholder];
    let rendered_remainder = content.strip_prefix(prefix)?;

    for (end, _) in rendered_remainder.match_indices(suffix) {
        let bin_dir = &rendered_remainder[..end];
        let Some(install_dir) = bin_dir.strip_suffix("/bin") else {
            continue;
        };
        let install_dir = PathBuf::from(install_dir);
        if script.content(&install_dir) == content {
            return Some(install_dir);
        }
    }

    None
}

fn managed_shell_script(name: &str) -> Option<ShellScript> {
    get_supported_shells().into_iter().find_map(|shell| {
        let script = shell.env_script();
        (script.name == name).then_some(script)
    })
}

pub(crate) fn remove_managed_shell_scripts(
    install_root: &Dir,
    target_dir: &Path,
    configured_target_dir: &Path,
) -> ManagedShellScriptCleanup {
    remove_managed_shell_scripts_with(
        install_root,
        target_dir,
        configured_target_dir,
        cap_managed_shell_script_install_path,
    )
}

fn remove_managed_shell_scripts_with<F>(
    install_root: &Dir,
    target_dir: &Path,
    configured_target_dir: &Path,
    mut classify: F,
) -> ManagedShellScriptCleanup
where
    F: FnMut(&Dir, &str, &Path, &Path) -> std::io::Result<Option<PathBuf>>,
{
    let mut configured_paths = Vec::new();
    let mut cleanup_error = None;

    for name in ["env", "env.fish", "env.nu"] {
        let quarantine = match quarantine_entry(install_root, name) {
            Ok(Some(quarantine)) => quarantine,
            Ok(None) => continue,
            Err(error) => {
                remember_shell_cleanup_error(&mut cleanup_error, error, name);
                continue;
            }
        };
        let configured_path = match classify(&quarantine, name, target_dir, configured_target_dir) {
            Ok(configured_path) => configured_path,
            Err(error) => {
                let error = add_restore_error(
                    error,
                    restore_quarantined_entry(quarantine, install_root, name),
                );
                remember_shell_cleanup_error(&mut cleanup_error, error, name);
                continue;
            }
        };

        if let Some(configured_path) = configured_path {
            if !configured_paths.contains(&configured_path) {
                configured_paths.push(configured_path);
            }
            if let Err(error) = quarantine.remove_file("entry") {
                let error = add_restore_error(
                    error,
                    restore_quarantined_entry(quarantine, install_root, name),
                );
                remember_shell_cleanup_error(&mut cleanup_error, error, name);
            } else if let Err(error) = quarantine.close() {
                remember_shell_cleanup_error(&mut cleanup_error, error, name);
            }
        } else if let Err(error) = restore_quarantined_entry(quarantine, install_root, name) {
            remember_shell_cleanup_error(&mut cleanup_error, error, name);
        } else {
            tracing::debug!(%name, "Preserving unowned env script");
        }
    }

    ManagedShellScriptCleanup {
        configured_paths,
        result: cleanup_error.map_or(Ok(()), |error| Err(error.into())),
    }
}

fn remember_shell_cleanup_error(
    cleanup_error: &mut Option<std::io::Error>,
    error: std::io::Error,
    name: &str,
) {
    if cleanup_error.is_none() {
        *cleanup_error = Some(error);
    } else {
        tracing::warn!(%error, %name, "Additional shell script cleanup failure");
    }
}

fn add_restore_error(primary: std::io::Error, restore: std::io::Result<()>) -> std::io::Error {
    let Err(restore) = restore else {
        return primary;
    };
    std::io::Error::new(
        primary.kind(),
        format!("{primary}; failed to restore quarantined shell script: {restore}"),
    )
}

fn create_shell_script_staging(staging_root: &Dir) -> std::io::Result<cap_tempfile::TempDir> {
    cap_tempfile::tempdir_in(staging_root)
}

fn cap_managed_shell_script_install_path(
    dir: &Dir,
    name: &str,
    target_dir: &Path,
    configured_target_dir: &Path,
) -> std::io::Result<Option<PathBuf>> {
    let metadata = match dir.symlink_metadata("entry") {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    if metadata.file_type().is_symlink()
        || !metadata.is_file()
        || metadata.len() > MAX_MANAGED_SHELL_SCRIPT_BYTES
    {
        return Ok(None);
    }

    let mut options = CapOpenOptions::new();
    options.read(true)._cap_fs_ext_follow(FollowSymlinks::No);
    let file = match dir.open_with("entry", &options) {
        Ok(file) => file,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    let metadata = file.metadata()?;
    if !metadata.is_file() || metadata.len() > MAX_MANAGED_SHELL_SCRIPT_BYTES {
        return Ok(None);
    }

    let mut content = Vec::with_capacity(metadata.len() as usize);
    file.take(MAX_MANAGED_SHELL_SCRIPT_BYTES + 1)
        .read_to_end(&mut content)?;
    if content.len() as u64 > MAX_MANAGED_SHELL_SCRIPT_BYTES {
        return Ok(None);
    }
    let Ok(content) = String::from_utf8(content) else {
        return Ok(None);
    };
    let Some(script) = managed_shell_script(name) else {
        return Ok(None);
    };
    if !shell_path_is_ambiguous(target_dir) && content == script.content(target_dir) {
        return Ok(Some(target_dir.to_path_buf()));
    }
    let Some(configured_path) = managed_shell_script_install_path(name, &content) else {
        return Ok(None);
    };
    if shell_path_is_ambiguous(&configured_path) {
        return Ok(None);
    }

    // Older releases wrote the caller's relative spelling into the generated
    // script. That spelling is not globally unique: two installs created from
    // different working directories can both contain `./install`. Only accept
    // it when the removal request uses the same normalized relative spelling;
    // otherwise a copied script could be mistaken for one owned by this root.
    // Absolute paths still have to identify this install root.
    let belongs_to_install = if configured_path.is_relative() {
        legacy_relative_paths_match(&configured_path, configured_target_dir)
    } else {
        absolute_path_matches_target(&configured_path, target_dir)
    };
    Ok(belongs_to_install.then_some(configured_path))
}

fn absolute_path_matches_target(configured_path: &Path, target_dir: &Path) -> bool {
    let (Ok(configured_path), Ok(target_dir)) = (
        normalize_absolute_path(configured_path),
        normalize_absolute_path(target_dir),
    ) else {
        return false;
    };

    resolve_trusted_root_alias(&configured_path) == resolve_trusted_root_alias(&target_dir)
}

fn normalize_legacy_relative_path(path: &Path) -> Option<PathBuf> {
    if !path.is_relative() {
        return None;
    }

    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            std::path::Component::Normal(name) => normalized.push(name),
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir if normalized.pop() => {}
            std::path::Component::ParentDir
            | std::path::Component::Prefix(_)
            | std::path::Component::RootDir => return None,
        }
    }
    (!normalized.as_os_str().is_empty()).then_some(normalized)
}

fn legacy_relative_paths_match(configured_path: &Path, requested_path: &Path) -> bool {
    matches!(
        (
            normalize_legacy_relative_path(configured_path),
            normalize_legacy_relative_path(requested_path),
        ),
        (Some(configured), Some(requested)) if configured == requested
    )
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
        validate_shell_configuration_path(install_dir)?;
        let env_path = install_dir.join(script.name);
        let env_content = script.content(install_dir);

        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(env_path)?;

        file.write_all(env_content.as_bytes())?;
        file.sync_data()?;

        Ok(())
    }

    fn write_script_in(
        &self,
        install_root: &Dir,
        staging_root: &Dir,
        script: &ShellScript,
        install_dir: &Path,
    ) -> Result<()> {
        validate_shell_configuration_path(install_dir)?;
        let env_content = script.content(install_dir);
        // Keep crash leftovers inside a managed version directory so removing
        // that version also reclaims an interrupted script publication.
        let staging = create_shell_script_staging(staging_root)?;
        let mut options = CapOpenOptions::new();
        options
            .write(true)
            .create_new(true)
            ._cap_fs_ext_follow(FollowSymlinks::No);
        let mut file = staging.open_with("entry", &options)?;
        file.write_all(env_content.as_bytes())?;
        file.sync_data()?;
        drop(file);
        staging.rename("entry", install_root, script.name)?;
        crate::fs::sync_cap_directory(install_root)?;
        if let Err(error) = staging.close() {
            tracing::warn!(%error, script = script.name, "Failed to remove empty script staging directory");
        }
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
    fn zdotdir() -> Option<PathBuf> {
        match std::env::var("ZDOTDIR") {
            Ok(dir) if !dir.is_empty() => Some(PathBuf::from(dir)),
            _ => None,
        }
    }
}

impl UnixShell for Zsh {
    fn is_present(&self) -> bool {
        matches!(std::env::var("SHELL"), Ok(sh) if sh.ends_with("/zsh"))
            || is_command_in_path("zsh")
    }

    fn potential_rc_paths(&self) -> Vec<PathBuf> {
        let home_env = std::env::var("HOME").ok().map(PathBuf::from);
        [Zsh::zdotdir(), home_env]
            .iter()
            .filter_map(|dir| dir.as_ref().map(|p| p.join(".zshenv")))
            .collect()
    }

    fn effective_rc_files(&self) -> Vec<PathBuf> {
        let candidates = self.potential_rc_paths();

        // Prefer all existing rc files so we update/remove entries everywhere.
        let existing: Vec<PathBuf> = candidates
            .iter()
            .filter(|rc| rc.is_file())
            .cloned()
            .collect();

        if !existing.is_empty() {
            return existing;
        }

        // If none exist, fall back to the first potential path (to create on install).
        candidates.into_iter().take(1).collect()
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
        xdg_config_path(&["fish", "conf.d", "wasmedgeup.fish"])
            .into_iter()
            .collect()
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
        xdg_config_path(&["nushell", "config.nu"])
            .into_iter()
            .collect()
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

    writeln!(file, "{line}")?;

    file.sync_data()?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generated_script_path_round_trips_for_every_supported_format() {
        let install_dir = Path::new("./relative install");

        for name in ["env", "env.fish", "env.nu"] {
            let content = managed_shell_script_content(name, install_dir).unwrap();
            assert_eq!(
                managed_shell_script_install_path(name, &content).as_deref(),
                Some(install_dir),
                "failed to recover the install path from {name}"
            );
        }
    }

    #[test]
    fn later_script_error_returns_paths_recovered_from_removed_scripts() {
        let parent = tempfile::tempdir().unwrap();
        let install_path = std::fs::canonicalize(parent.path())
            .unwrap()
            .join("legacy-root");
        std::fs::create_dir(&install_path).unwrap();
        let configured_path = Path::new("./legacy-root");
        for name in ["env", "env.fish"] {
            let content = managed_shell_script_content(name, configured_path).unwrap();
            std::fs::write(install_path.join(name), content).unwrap();
        }
        let install_root = open_dir_nofollow(&install_path).unwrap();

        let cleanup = remove_managed_shell_scripts_with(
            &install_root,
            &install_path,
            configured_path,
            |dir, name, target_dir, configured_target_dir| {
                if name == "env.fish" {
                    Err(std::io::Error::other("simulated read failure"))
                } else {
                    cap_managed_shell_script_install_path(
                        dir,
                        name,
                        target_dir,
                        configured_target_dir,
                    )
                }
            },
        );

        assert_eq!(
            cleanup.configured_paths,
            vec![configured_path.to_path_buf()]
        );
        assert!(cleanup.result.is_err());
        assert!(
            !install_path.join("env").exists(),
            "the earlier managed script should already be removed"
        );
        assert!(
            install_path.join("env.fish").is_file(),
            "the script that failed inspection must be restored"
        );

        let retry = remove_managed_shell_scripts(&install_root, &install_path, configured_path);
        retry.result.unwrap();
        assert_eq!(retry.configured_paths, vec![configured_path.to_path_buf()]);
        for name in ["env", "env.fish"] {
            assert!(
                !install_path.join(name).exists(),
                "a successful retry must remove {name}"
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn lossy_non_utf8_script_collision_is_not_classified_as_managed() {
        use std::os::unix::ffi::OsStringExt;

        let parent = tempfile::tempdir().unwrap();
        let install_path =
            PathBuf::from(std::ffi::OsString::from_vec(b"/tmp/install-\xff".to_vec()));
        let other_path = PathBuf::from(std::ffi::OsString::from_vec(b"/tmp/install-\xfe".to_vec()));
        let install_content = managed_shell_script_content("env", &install_path).unwrap();
        let other_content = managed_shell_script_content("env", &other_path).unwrap();
        assert_eq!(
            install_content, other_content,
            "the legacy lossy renderer must demonstrate the ownership collision"
        );
        std::fs::write(parent.path().join("entry"), other_content).unwrap();
        let quarantine = open_dir_nofollow(parent.path()).unwrap();

        let configured_path =
            cap_managed_shell_script_install_path(&quarantine, "env", &install_path, &install_path)
                .unwrap();

        assert_eq!(configured_path, None);
    }

    #[cfg(unix)]
    #[test]
    fn setup_rejects_ambiguous_or_shell_unsafe_paths() {
        use std::os::unix::ffi::OsStringExt;

        let mut rejected_paths = vec![
            PathBuf::from(std::ffi::OsString::from_vec(b"/tmp/install-\xff".to_vec())),
            PathBuf::from("/tmp/install-\u{fffd}"),
        ];
        rejected_paths.extend(
            ['"', '\\', '$', '`', '(', ')', ':', '\n', '\r', '\t']
                .map(|character| PathBuf::from(format!("/tmp/install-{character}unsafe"))),
        );

        for install_path in rejected_paths {
            let result = setup_path(&install_path);
            assert!(
                matches!(result, Err(Error::InvalidPath { .. })),
                "unsafe path was accepted: {install_path:?}"
            );
        }
    }

    #[test]
    fn validation_accepts_quoted_shell_literals_without_expansion_syntax() {
        let install_path = Path::new("/tmp/WasmEdge dev!; #1 &'測試' @+%=,[]{}~");
        assert!(validate_shell_configuration_path(install_path).is_ok());
    }

    #[cfg(target_vendor = "apple")]
    #[test]
    fn generated_script_matches_a_macos_trusted_root_alias() {
        let parent = tempfile::tempdir().unwrap();
        let configured_path = Path::new("/tmp/wasmedgeup-alias-test");
        let target_path = Path::new("/private/tmp/wasmedgeup-alias-test");
        let content = managed_shell_script_content("env", configured_path).unwrap();
        std::fs::write(parent.path().join("entry"), content).unwrap();
        let quarantine = open_dir_nofollow(parent.path()).unwrap();

        let recovered =
            cap_managed_shell_script_install_path(&quarantine, "env", target_path, configured_path)
                .unwrap();

        assert_eq!(recovered.as_deref(), Some(configured_path));
    }

    #[test]
    fn pinned_script_write_ignores_an_ambient_path_replacement() {
        use std::os::unix::fs::symlink;

        let parent = tempfile::tempdir().unwrap();
        let install_path = parent.path().join("install");
        let pinned_path = parent.path().join("pinned-install");
        std::fs::create_dir(&install_path).unwrap();
        let install_root = open_dir_nofollow(&install_path).unwrap();

        let outside = tempfile::tempdir().unwrap();
        std::fs::rename(&install_path, &pinned_path).unwrap();
        symlink(outside.path(), &install_path).unwrap();

        let shell = Posix;
        let script = shell.env_script();
        let staging_path = pinned_path.join("managed-version");
        std::fs::create_dir(&staging_path).unwrap();
        let staging_root = open_dir_nofollow(&staging_path).unwrap();
        shell
            .write_script_in(&install_root, &staging_root, &script, &install_path)
            .unwrap();

        assert!(pinned_path.join("env").is_file());
        assert!(
            !outside.path().join("env").exists(),
            "script writes must remain anchored to the opened install root"
        );
        assert_eq!(
            staging_root.entries().unwrap().count(),
            0,
            "successful publication must remove its managed staging directory"
        );
    }

    #[test]
    fn shell_script_staging_is_contained_by_managed_version() {
        use crate::constants::{VERSION_INSTALL_MARKER, VERSION_INSTALL_MARKER_CONTENT};

        let install = tempfile::tempdir().unwrap();
        let version_path = install.path().join("versions/0.14.1");
        std::fs::create_dir_all(&version_path).unwrap();
        std::fs::write(
            version_path.join(VERSION_INSTALL_MARKER),
            VERSION_INSTALL_MARKER_CONTENT,
        )
        .unwrap();
        let install_root = open_dir_nofollow(install.path()).unwrap();
        let version_root = open_dir_nofollow(&version_path).unwrap();

        let staging = create_shell_script_staging(&version_root).unwrap();
        let _persisted_staging = staging.into_dir().unwrap();

        let root_entries: Vec<_> = install_root
            .entries()
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect();
        assert_eq!(root_entries, vec![std::ffi::OsString::from("versions")]);
        let version_entries: Vec<_> = version_root
            .entries()
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect();
        assert_eq!(version_entries.len(), 2);
        assert!(version_entries.contains(&std::ffi::OsString::from(VERSION_INSTALL_MARKER)));
    }
}

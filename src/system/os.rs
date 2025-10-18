use crate::system::spec::{LibcKind, LibcSpec, OsSpec};
use crate::target::{TargetArch, TargetOS};
use std::fs;
#[cfg(unix)]
use std::process::Command;

pub fn detect_os() -> (OsSpec, Vec<String>, Vec<String>) {
    #[cfg(unix)]
    {
        detect_os_unix()
    }
    #[cfg(windows)]
    {
        detect_os_windows()
    }
}

#[cfg(unix)]
fn detect_os_unix() -> (OsSpec, Vec<String>, Vec<String>) {
    let notes = Vec::new();
    let mut errors = Vec::new();

    let os_type = TargetOS::default();
    let arch = TargetArch::default();

    let (distro, version) = read_os_release().unwrap_or_else(|e| {
        errors.push(format!("os-release: {e}"));
        (None, None)
    });

    let kernel = uname_kernel().unwrap_or_else(|e| {
        errors.push(format!("uname: {e}"));
        None
    });

    let libc = detect_libc().unwrap_or_else(|e| {
        errors.push(format!("libc: {e}"));
        LibcSpec {
            kind: LibcKind::Unknown,
            version: None,
        }
    });

    let os = OsSpec {
        os_type,
        arch,
        distro,
        version,
        kernel,
        libc,
    };
    (os, notes, errors)
}

#[cfg(windows)]
fn detect_os_windows() -> (OsSpec, Vec<String>, Vec<String>) {
    let notes = Vec::new();
    let errors = Vec::new();

    let os_type = TargetOS::default();
    let arch = TargetArch::default();

    let distro = Some("Windows".to_string());
    let version = None;
    let kernel = None;
    let libc = LibcSpec {
        kind: LibcKind::Unknown,
        version: None,
    };

    let os = OsSpec {
        os_type,
        arch,
        distro,
        version,
        kernel,
        libc,
    };
    (os, notes, errors)
}

#[cfg(unix)]
fn read_os_release() -> Result<(Option<String>, Option<String>), String> {
    let content = fs::read_to_string("/etc/os-release").map_err(|e| e.to_string())?;
    let mut name: Option<String> = None;
    let mut version: Option<String> = None;
    for line in content.lines() {
        let line = line.trim();
        if line.starts_with('#') || line.is_empty() {
            continue;
        }
        if let Some((k, v)) = line.split_once('=') {
            let v = v.trim_matches('"');
            if (k == "NAME" || k == "ID") && name.is_none() {
                name = Some(v.to_string());
            }
            if k == "VERSION_ID" {
                version = Some(v.to_string());
            }
        }
    }
    Ok((name, version))
}

#[cfg(unix)]
fn uname_kernel() -> Result<Option<String>, String> {
    let out = Command::new("uname")
        .arg("-srm")
        .output()
        .map_err(|e| e.to_string())?;
    if out.status.success() {
        Ok(Some(
            String::from_utf8_lossy(&out.stdout).trim().to_string(),
        ))
    } else {
        Ok(None)
    }
}

#[cfg(unix)]
fn detect_libc() -> Result<LibcSpec, String> {
    let out = Command::new("ldd")
        .arg("--version")
        .output()
        .map_err(|e| e.to_string())?;
    if !out.status.success() {
        return Ok(LibcSpec {
            kind: LibcKind::Unknown,
            version: None,
        });
    }
    let stdout = String::from_utf8_lossy(&out.stdout);
    let s = stdout.to_lowercase();
    if s.contains("musl") {
        let ver = stdout
            .lines()
            .next()
            .and_then(|l| l.split_whitespace().last())
            .map(|s| s.to_string());
        Ok(LibcSpec {
            kind: LibcKind::Musl,
            version: ver,
        })
    } else if s.contains("glibc") || s.contains("gnu libc") || s.contains("gnu c library") {
        let ver: Option<String> = stdout
            .lines()
            .next()
            .and_then(|l| l.split_whitespace().last())
            .map(|s| s.to_string());
        Ok(LibcSpec {
            kind: LibcKind::Glibc,
            version: ver,
        })
    } else {
        Ok(LibcSpec {
            kind: LibcKind::Unknown,
            version: None,
        })
    }
}

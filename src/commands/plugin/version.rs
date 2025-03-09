use std::str::FromStr;

use semver::Version;

#[allow(dead_code)]
#[derive(Debug, Clone)]
pub enum PluginVersion {
    Name(String),
    NameAndVersion(String, Version),
}

impl FromStr for PluginVersion {
    type Err = semver::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let Some((name, ver_str)) = s.split_once('@') else {
            return Ok(Self::Name(s.to_string()));
        };

        let version = ver_str.parse()?;
        Ok(Self::NameAndVersion(name.to_string(), version))
    }
}

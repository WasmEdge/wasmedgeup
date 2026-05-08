use git2::{Direction, Remote, RemoteHead};
use semver::Version;
use snafu::ResultExt as _;

use crate::prelude::*;

#[derive(Debug, Clone, Copy)]
pub enum ReleasesFilter {
    All,
    Stable,
}

impl ReleasesFilter {
    pub fn matches(self, semver: &semver::Version) -> bool {
        match self {
            Self::All => true,
            Self::Stable => semver.pre.is_empty(),
        }
    }
}

/// Get all releases sorted from newest to oldest.
pub fn get_all(url: &str, filter: ReleasesFilter) -> Result<Vec<Version>> {
    let mut remote = Remote::create_detached(url).context(GitSnafu { resource: "remote" })?;
    remote.connect(Direction::Fetch).context(GitSnafu {
        resource: "remote/connect",
    })?;

    let list = remote.list().context(GitSnafu {
        resource: "remote/list",
    })?;
    let mut heads = list
        .iter()
        .filter_map(remote_head_to_version)
        .filter(|version| filter.matches(version))
        .collect::<Vec<_>>();
    heads.sort_unstable_by(|a, b| b.cmp(a));

    Ok(heads)
}

fn remote_head_to_version(head: &'_ RemoteHead<'_>) -> Option<Version> {
    parse_tag_ref(head.name())
}

/// Parse a fully-qualified ref name into a semver `Version` if it represents
/// a release tag we recognise. Returns `None` for non-tag refs, peeled tags
/// (`^{}` suffix), and tag names that don't parse as semver.
fn parse_tag_ref(ref_name: &str) -> Option<Version> {
    let name = ref_name.strip_prefix("refs/tags/")?;
    if name.ends_with("^{}") {
        return None;
    }
    Version::parse(name).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_simple_release_tag() {
        let v = parse_tag_ref("refs/tags/0.14.1").unwrap();
        assert_eq!(v, Version::new(0, 14, 1));
    }

    #[test]
    fn parses_prerelease_tag() {
        let v = parse_tag_ref("refs/tags/0.15.0-alpha.1").unwrap();
        assert_eq!(v.major, 0);
        assert_eq!(v.minor, 15);
        assert_eq!(v.patch, 0);
        assert_eq!(v.pre.as_str(), "alpha.1");
    }

    #[test]
    fn rejects_peeled_tag() {
        assert!(parse_tag_ref("refs/tags/0.14.1^{}").is_none());
    }

    #[test]
    fn rejects_non_tag_ref() {
        assert!(parse_tag_ref("refs/heads/master").is_none());
        assert!(parse_tag_ref("HEAD").is_none());
    }

    #[test]
    fn rejects_non_semver_tag() {
        assert!(parse_tag_ref("refs/tags/not-a-version").is_none());
        assert!(parse_tag_ref("refs/tags/v0.14.1").is_none());
    }

    #[test]
    fn rejects_empty() {
        assert!(parse_tag_ref("").is_none());
        assert!(parse_tag_ref("refs/tags/").is_none());
    }
}

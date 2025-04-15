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
    let name = head.name().strip_prefix("refs/tags/")?;
    if name.ends_with("^{}") {
        return None;
    }
    Version::parse(name).ok()
}

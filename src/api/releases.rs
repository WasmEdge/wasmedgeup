use crate::prelude::*;
use std::{
    pin::Pin,
    sync::OnceLock,
    task::{Context, Poll},
};

use futures::{future::BoxFuture, Stream};
use pin_project_lite::pin_project;
use regex::Regex;
use reqwest::StatusCode;
use snafu::ResultExt;

const RELEASES_URL: &str = "https://github.com/WasmEdge/WasmEdge/releases";

static RELEASE_TAG_REGEX: OnceLock<Regex> = OnceLock::new();

fn release_tag_regex() -> &'static Regex {
    RELEASE_TAG_REGEX.get_or_init(|| {
        Regex::new(r"releases\/tag\/(?<version>[0-9]+\.[0-9]+\.[0-9]+(\-[[:alpha:]]+\.[0-9]+)?)")
            .expect("release tag regex should be valid")
    })
}

pin_project! {
    pub struct Releases<'a> {
        client: reqwest::Client,
        filter: ReleasesFilter,
        current_page: usize,
        current_start: usize,

        #[pin]
        state: State<'a>
    }
}

enum State<'a> {
    Ready,
    Loading(BoxFuture<'a, reqwest::Result<String>>),
    Fetched(String),
}

impl<'a> Releases<'a> {
    pub fn new(client: reqwest::Client, filter: ReleasesFilter) -> Self {
        Self {
            client,
            filter,
            current_page: 1,
            current_start: 0,
            state: State::Ready,
        }
    }

    fn fetch_releases(&mut self, page: usize) -> BoxFuture<'a, reqwest::Result<String>> {
        let client = self.client.clone();

        Box::pin(async move {
            client
                .get(RELEASES_URL)
                .query(&[("page", page)])
                .send()
                .await?
                .text()
                .await
        })
    }
}

impl<'a> Stream for Releases<'a> {
    type Item = Result<semver::Version>;

    fn poll_next(self: Pin<&mut Releases<'a>>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = self.get_mut();

        loop {
            match &mut this.state {
                State::Ready => {
                    let fut = this.fetch_releases(this.current_page);
                    this.state = State::Loading(fut);
                }
                State::Loading(fut) => match futures::ready!(fut.as_mut().poll(cx)) {
                    Err(e) if matches!(e.status(), Some(StatusCode::NOT_FOUND)) => {
                        return Poll::Ready(None)
                    }
                    Err(e) => {
                        return Poll::Ready(Some(Err(e).context(GitHubSnafu {
                            resource: "releases",
                        })))
                    }
                    Ok(s) => {
                        this.state = State::Fetched(s);
                    }
                },
                State::Fetched(ref html) => {
                    let Some(caps) = release_tag_regex().captures_at(html, this.current_start)
                    else {
                        this.current_start = 0;
                        this.current_page += 1;
                        this.state = State::Ready;
                        continue;
                    };

                    let version = caps.name("version").unwrap();
                    let parsed_version = version.as_str().parse().context(SemVerSnafu {})?;
                    this.current_start = version.end();

                    if !this.filter.matches(&parsed_version) {
                        continue;
                    }

                    return Poll::Ready(Some(Ok(parsed_version)));
                }
            }
        }
    }
}

impl std::fmt::Debug for State<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Ready => write!(f, "Ready"),
            Self::Loading(_) => write!(f, "Loading(...)"),
            Self::Fetched(s) => f.debug_tuple("Fetched").field(s).finish(),
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub enum ReleasesFilter {
    All,
    Stable,
}

impl ReleasesFilter {
    fn matches(self, semver: &semver::Version) -> bool {
        match self {
            Self::All => true,
            Self::Stable => semver.pre.is_empty(),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::ops::Range;

    use rstest::rstest;

    use super::*;

    #[rstest]
    #[case::stable(r#"href="/WasmEdge/WasmEdge/releases/tag/1.0.0""#, "1.0.0", 38..43)]
    #[case::alpha(r#"href="/WasmEdge/WasmEdge/releases/tag/1.0.0-alpha.1""#, "1.0.0-alpha.1", 38..51)]
    #[case::beta(r#"href="/WasmEdge/WasmEdge/releases/tag/1.0.0-beta.1""#, "1.0.0-beta.1", 38..50)]
    #[case::rc(r#"href="/WasmEdge/WasmEdge/releases/tag/1.0.0-rc.1""#, "1.0.0-rc.1", 38..48)]
    #[case::any_prerelease(r#"href="/WasmEdge/WasmEdge/releases/tag/1.0.0-anyprerelease.1""#, "1.0.0-anyprerelease.1", 38..59)]
    fn test_valid_semver_valid_release_tags(
        #[case] input: &str,
        #[case] expected_str: &str,
        #[case] expected_range: Range<usize>,
    ) {
        let result = release_tag_regex().captures(input).unwrap();
        let version = result.name("version").unwrap();
        assert_eq!(version.as_str(), expected_str);
        assert_eq!(version.range(), expected_range);
    }

    #[rstest]
    #[case::prerelease_no_num(r#"href="/WasmEdge/WasmEdge/releases/tag/1.0.0-alpha""#, "1.0.0", 38..43)]
    #[case::prerelease_no_alpha(r#"href="/WasmEdge/WasmEdge/releases/tag/1.0.0-0.3.7""#, "1.0.0", 38..43)]
    #[case::prerelease_multiple(r#"href="/WasmEdge/WasmEdge/releases/tag/1.0.0-x.7.z.92""#, "1.0.0-x.7", 38..47)]
    #[case::prerelease_hyphens(r#"href="/WasmEdge/WasmEdge/releases/tag/1.0.0-x-y-z.--""#, "1.0.0", 38..43)]
    #[case::build_meta(r#"href="/WasmEdge/WasmEdge/releases/tag/1.0.0+20130313144700""#, "1.0.0", 38..43)]
    #[case::prerelease_build_meta(r#"href="/WasmEdge/WasmEdge/releases/tag/1.0.0-alpha+001""#, "1.0.0", 38..43)]
    fn test_valid_semver_partial_release_tags(
        #[case] input: &str,
        #[case] expected_str: &str,
        #[case] expected_range: Range<usize>,
    ) {
        let result = release_tag_regex().captures(input).unwrap();
        let version = result.name("version").unwrap();
        assert_eq!(version.as_str(), expected_str);
        assert_eq!(version.range(), expected_range);
    }
}

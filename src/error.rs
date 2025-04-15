use snafu::Snafu;

#[derive(Debug, Default, Snafu)]
#[snafu(visibility(pub))]
pub enum Error {
    #[snafu(display("Unable to fetch resource '{}' for git", resource))]
    Git {
        source: git2::Error,
        resource: &'static str,
    },

    #[snafu(display("Invalid semantic version specifier"))]
    SemVer { source: semver::Error },

    #[default]
    #[snafu(display("Unknown error occurred"))]
    Unknown,
}

pub type Result<T, E = Error> = std::result::Result<T, E>;

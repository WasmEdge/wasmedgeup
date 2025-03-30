use snafu::Snafu;

#[derive(Debug, Snafu)]
#[snafu(visibility(pub))]
pub enum Error {
    #[snafu(display("Unable to fetch resource '{}' from GitHub API", resource))]
    GitHub {
        source: octocrab::Error,
        resource: &'static str,
    },
}

pub type Result<T, E = Error> = std::result::Result<T, E>;

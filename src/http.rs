use crate::prelude::*;
use reqwest::Client;
use std::time::Duration;

/// Configuration for building HTTP clients with consistent settings.
#[derive(Debug, Clone)]
pub struct HttpClientConfig {
    /// Connection timeout in seconds
    pub connect_timeout: u64,
    /// Request timeout in seconds
    pub request_timeout: u64,
}

impl Default for HttpClientConfig {
    fn default() -> Self {
        Self {
            connect_timeout: 15, // 15 seconds for connection
            request_timeout: 90, // 90 seconds for request
        }
    }
}

impl HttpClientConfig {
    /// Creates a new HTTP client config with default timeouts.
    pub fn new() -> Self {
        Self::default()
    }

    /// Sets the connection timeout in seconds.
    pub fn with_connect_timeout(mut self, timeout: u64) -> Self {
        self.connect_timeout = timeout;
        self
    }

    /// Sets the request timeout in seconds.
    pub fn with_request_timeout(mut self, timeout: u64) -> Self {
        self.request_timeout = timeout;
        self
    }

    /// Builds a reqwest Client with the configured settings.
    pub fn build(&self) -> Result<Client> {
        reqwest::ClientBuilder::new()
            .connect_timeout(Duration::from_secs(self.connect_timeout))
            .timeout(Duration::from_secs(self.request_timeout))
            .user_agent(format!(
                "wasmedgeup/{} (+https://github.com/WasmEdge/wasmedgeup)",
                env!("CARGO_PKG_VERSION")
            ))
            .build()
            .map_err(|e| Error::HttpClientBuild {
                reason: e.to_string(),
            })
    }
}

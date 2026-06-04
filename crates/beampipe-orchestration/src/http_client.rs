//! Shared `reqwest` client options for TM/DIM HTTP clients.

use std::time::Duration;

#[derive(Debug, Clone)]
pub struct HttpClientOptions {
    pub verify_ssl: bool,
    pub timeout_secs: u64,
}

impl Default for HttpClientOptions {
    fn default() -> Self {
        Self {
            verify_ssl: true,
            timeout_secs: 60,
        }
    }
}

impl HttpClientOptions {
    pub fn translator_default() -> Self {
        Self {
            verify_ssl: true,
            timeout_secs: 120,
        }
    }

    pub fn dim_default() -> Self {
        Self {
            verify_ssl: true,
            timeout_secs: 60,
        }
    }

    pub fn with_verify_ssl(mut self, verify_ssl: bool) -> Self {
        self.verify_ssl = verify_ssl;
        self
    }
}

pub fn build_http_client(opts: &HttpClientOptions) -> reqwest::Client {
    let mut builder = reqwest::Client::builder().timeout(Duration::from_secs(opts.timeout_secs));
    if !opts.verify_ssl {
        builder = builder.danger_accept_invalid_certs(true);
    }
    builder.build().unwrap_or_default()
}

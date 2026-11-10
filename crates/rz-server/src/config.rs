// SPDX-FileCopyrightText: 2026 The Rundenzeit Contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Server configuration, from command-line flags or `RZ_*` environment
//! variables (docs/SERVER.md).

use std::net::SocketAddr;
use std::path::PathBuf;

use base64::Engine;
use clap::Args;

/// Settings of a running server.
#[derive(Debug, Clone, Args)]
pub struct Config {
    /// Postgres connection URL
    #[arg(long, env = "RZ_DATABASE_URL")]
    pub database_url: String,

    /// Public base URL of the server as users reach it, e.g.
    /// `https://laps.example.com`. Used for cookie security, CSRF origin
    /// checks and (later) passkeys.
    #[arg(long, env = "RZ_PUBLIC_URL", default_value = "http://localhost:8080")]
    pub public_url: String,

    /// Secret key, 32 bytes as base64 (`openssl rand -base64 32`). Encrypts
    /// TOTP secrets; changing it invalidates all TOTP setups.
    #[arg(long, env = "RZ_SECRET_KEY", hide_env_values = true)]
    pub secret_key: String,

    /// Directory for telemetry files
    #[arg(long, env = "RZ_DATA_DIR", default_value = "./data")]
    pub data_dir: PathBuf,

    /// Directory with the built web UI (`web/build`); not served if missing
    #[arg(long, env = "RZ_WEB_DIR", default_value = "./web/build")]
    pub web_dir: PathBuf,

    /// Address to listen on
    #[arg(long, env = "RZ_LISTEN", default_value = "0.0.0.0:8080")]
    pub listen: SocketAddr,

    /// Take the client address from `X-Forwarded-For` (only behind a
    /// reverse proxy that sets it)
    #[arg(long, env = "RZ_TRUST_PROXY", default_value_t = false)]
    pub trust_proxy: bool,

    /// Largest accepted upload (.rzx import), MiB
    #[arg(long, env = "RZ_MAX_UPLOAD_MB", default_value_t = 512)]
    pub max_upload_mb: u64,

    /// Username of the first admin, created at startup if there are no users
    #[arg(long, env = "RZ_ADMIN_USERNAME")]
    pub admin_username: Option<String>,

    /// Password of the first admin (only used when it is created)
    #[arg(long, env = "RZ_ADMIN_PASSWORD", hide_env_values = true)]
    pub admin_password: Option<String>,
}

/// Invalid configuration.
#[derive(Debug, thiserror::Error)]
#[error("configuration: {0}")]
pub struct ConfigError(pub String);

impl Config {
    /// The decoded secret key.
    pub fn secret_key_bytes(&self) -> Result<[u8; 32], ConfigError> {
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(self.secret_key.trim())
            .map_err(|_| ConfigError("RZ_SECRET_KEY is not valid base64".into()))?;
        bytes.try_into().map_err(|_| {
            ConfigError("RZ_SECRET_KEY must be 32 bytes (openssl rand -base64 32)".into())
        })
    }

    /// True if users reach the server over HTTPS (secure cookies).
    pub fn is_https(&self) -> bool {
        self.public_url.starts_with("https://")
    }

    /// `scheme://host[:port]` of the public URL, for Origin checks.
    pub fn public_origin(&self) -> String {
        let url = self.public_url.trim_end_matches('/');
        match url.find("://") {
            Some(i) => match url[i + 3..].find('/') {
                Some(j) => url[..i + 3 + j].to_owned(),
                None => url.to_owned(),
            },
            None => url.to_owned(),
        }
    }

    /// Checks values that clap can't.
    pub fn validate(&self) -> Result<(), ConfigError> {
        self.secret_key_bytes()?;
        if !(self.public_url.starts_with("http://") || self.public_url.starts_with("https://")) {
            return Err(ConfigError(
                "RZ_PUBLIC_URL must start with http:// or https://".into(),
            ));
        }
        Ok(())
    }

    /// A configuration for tests.
    #[doc(hidden)]
    pub fn for_tests(database_url: String, data_dir: PathBuf) -> Self {
        Self {
            database_url,
            public_url: "http://localhost:8080".into(),
            secret_key: base64::engine::general_purpose::STANDARD.encode([7u8; 32]),
            data_dir,
            web_dir: PathBuf::from("/nonexistent"),
            listen: "127.0.0.1:0".parse().expect("valid"),
            trust_proxy: false,
            max_upload_mb: 64,
            admin_username: None,
            admin_password: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn public_origin_strips_path() {
        let mut c = Config::for_tests(String::new(), PathBuf::new());
        c.public_url = "https://laps.example.com/rz/".into();
        assert_eq!(c.public_origin(), "https://laps.example.com");
        c.public_url = "http://localhost:8080".into();
        assert_eq!(c.public_origin(), "http://localhost:8080");
        assert!(c.validate().is_ok());
        c.secret_key = "short".into();
        assert!(c.validate().is_err());
    }
}

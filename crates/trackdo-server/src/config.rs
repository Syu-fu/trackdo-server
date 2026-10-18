//! Settings read from the environment. See `docs/design.md`.

use std::net::SocketAddr;
use std::path::PathBuf;

use axum::http::{HeaderValue, Uri};
use thiserror::Error;

use crate::auth::TokenDigest;

const DEFAULT_LISTEN: &str = "127.0.0.1:8080";
const DEFAULT_DATABASE: &str = "trackdo.db";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    pub listen: SocketAddr,
    pub database: PathBuf,
    pub tokens: Vec<TokenDigest>,
    /// Web origins allowed to call the API from a browser.
    pub cors_origins: Vec<HeaderValue>,
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum ConfigError {
    #[error("TRACKDO_LISTEN is not an address and port: {0:?}")]
    InvalidListen(String),
    #[error("TRACKDO_TOKEN_HASHES must list at least one SHA-256 digest")]
    MissingTokens,
    /// Counted from 1, so the digest itself never reaches the log.
    #[error("TRACKDO_TOKEN_HASHES entry {0} is not a hex SHA-256 digest")]
    InvalidTokenDigest(usize),
    #[error("TRACKDO_CORS_ORIGINS entry is not an origin like https://app.example: {0:?}")]
    InvalidCorsOrigin(String),
}

impl Config {
    /// Reads the settings through `var`, e.g. `|name| std::env::var(name).ok()`.
    pub fn from_vars(var: impl Fn(&str) -> Option<String>) -> Result<Self, ConfigError> {
        let listen = var("TRACKDO_LISTEN").unwrap_or_else(|| DEFAULT_LISTEN.to_owned());
        let tokens = list(var("TRACKDO_TOKEN_HASHES").as_deref())
            .zip(1..)
            .map(|(hex, entry)| {
                TokenDigest::from_hex(hex).ok_or(ConfigError::InvalidTokenDigest(entry))
            })
            .collect::<Result<Vec<_>, _>>()?;
        if tokens.is_empty() {
            return Err(ConfigError::MissingTokens);
        }
        Ok(Self {
            listen: listen
                .parse()
                .map_err(|_| ConfigError::InvalidListen(listen.clone()))?,
            database: var("TRACKDO_DATABASE")
                .map_or_else(|| PathBuf::from(DEFAULT_DATABASE), PathBuf::from),
            tokens,
            cors_origins: list(var("TRACKDO_CORS_ORIGINS").as_deref())
                .map(origin)
                .collect::<Result<_, _>>()?,
        })
    }
}

/// An `http(s)://host[:port]` origin, exactly as browsers send it.
fn origin(entry: &str) -> Result<HeaderValue, ConfigError> {
    let invalid = || ConfigError::InvalidCorsOrigin(entry.to_owned());
    let uri: Uri = entry.parse().map_err(|_| invalid())?;
    let scheme = uri
        .scheme_str()
        .filter(|scheme| matches!(*scheme, "http" | "https"))
        .ok_or_else(invalid)?;
    let authority = uri.authority().ok_or_else(invalid)?;
    // Anything beyond scheme and authority, even a trailing slash, would
    // never match the Origin header.
    if format!("{scheme}://{authority}") != entry {
        return Err(invalid());
    }
    HeaderValue::from_str(entry).map_err(|_| invalid())
}

/// Comma-separated entries, trimmed, with empty ones dropped.
fn list(value: Option<&str>) -> impl Iterator<Item = &str> {
    value
        .unwrap_or_default()
        .split(',')
        .map(str::trim)
        .filter(|entry| !entry.is_empty())
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;

    const DIGEST: &str = "2bb80d537b1da3e38bd30361aa855686bde0eacd7162fef6a25fe97bf527a25b";
    const OTHER: &str = "a665a45920422f9d417e4867efdc4fb8a04a1f3fff1fa07e998e86f7f7a27ae3";

    fn config(vars: &[(&str, &str)]) -> Result<Config, ConfigError> {
        let vars: HashMap<String, String> = vars
            .iter()
            .map(|(name, value)| ((*name).to_owned(), (*value).to_owned()))
            .collect();
        Config::from_vars(|name| vars.get(name).cloned())
    }

    fn digest(hex: &str) -> TokenDigest {
        TokenDigest::from_hex(hex).unwrap()
    }

    #[test]
    fn defaults_everything_but_the_tokens() {
        let config = config(&[("TRACKDO_TOKEN_HASHES", DIGEST)]).unwrap();

        assert_eq!(
            config,
            Config {
                listen: "127.0.0.1:8080".parse().unwrap(),
                database: PathBuf::from("trackdo.db"),
                tokens: vec![digest(DIGEST)],
                cors_origins: Vec::new(),
            }
        );
    }

    #[test]
    fn reads_every_setting() {
        let config = config(&[
            ("TRACKDO_LISTEN", "0.0.0.0:9000"),
            ("TRACKDO_DATABASE", "/data/trackdo.db"),
            ("TRACKDO_TOKEN_HASHES", &format!("{DIGEST},{OTHER}")),
            (
                "TRACKDO_CORS_ORIGINS",
                "https://app.example, http://localhost:5173",
            ),
        ])
        .unwrap();

        assert_eq!(config.listen, "0.0.0.0:9000".parse().unwrap());
        assert_eq!(config.database, PathBuf::from("/data/trackdo.db"));
        assert_eq!(config.tokens, [digest(DIGEST), digest(OTHER)]);
        assert_eq!(
            config.cors_origins,
            ["https://app.example", "http://localhost:5173"]
        );
    }

    #[test]
    fn rejects_anything_but_an_origin() {
        for entry in [
            "app.example",
            "https://app.example/",
            "https://app.example/path",
            "ftp://app.example",
            "https://",
            "https://app.example?query",
        ] {
            let result = config(&[
                ("TRACKDO_TOKEN_HASHES", DIGEST),
                ("TRACKDO_CORS_ORIGINS", entry),
            ]);

            assert_eq!(
                result,
                Err(ConfigError::InvalidCorsOrigin(entry.to_owned())),
                "{entry}"
            );
        }
    }

    #[test]
    fn tolerates_spaces_and_empty_entries_in_lists() {
        let config = config(&[("TRACKDO_TOKEN_HASHES", &format!(" {DIGEST} ,, {OTHER},"))]);

        assert_eq!(config.unwrap().tokens, [digest(DIGEST), digest(OTHER)]);
    }

    #[test]
    fn requires_tokens() {
        assert_eq!(config(&[]), Err(ConfigError::MissingTokens));
        assert_eq!(
            config(&[("TRACKDO_TOKEN_HASHES", " , ")]),
            Err(ConfigError::MissingTokens)
        );
    }

    #[test]
    fn names_the_malformed_digest_without_echoing_it() {
        let result = config(&[("TRACKDO_TOKEN_HASHES", &format!("{DIGEST},secret"))]);

        assert_eq!(result, Err(ConfigError::InvalidTokenDigest(2)));
        assert!(!result.unwrap_err().to_string().contains("secret"));
    }

    #[test]
    fn rejects_a_malformed_listen_address() {
        let result = config(&[
            ("TRACKDO_LISTEN", "localhost"),
            ("TRACKDO_TOKEN_HASHES", DIGEST),
        ]);

        assert_eq!(
            result,
            Err(ConfigError::InvalidListen("localhost".to_owned()))
        );
    }
}

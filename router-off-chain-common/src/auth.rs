//! Request authentication middleware.
//!
//! Supports API key-based authentication via:
//! - `Authorization: Bearer <api-key>` header
//! - `X-API-Key: <api-key>` header
//!
//! ## Configuration (environment variables)
//!
//! | Variable | Default | Description |
//! |----------|---------|-------------|
//! | `ROUTER_API_KEY` | — | API key for authentication. Required when `ROUTER_AUTH_ENABLED=true`. |
//! | `ROUTER_AUTH_ENABLED` | `false` | Set to `"true"` to require authentication. |

use anyhow::{bail, Result};
use axum::{
    extract::Request,
    http::{HeaderMap, StatusCode},
    middleware::Next,
    response::{IntoResponse, Response},
};
use std::env;
use subtle::ConstantTimeEq;

/// Authentication configuration.
#[derive(Clone, Debug)]
pub struct AuthConfig {
    /// API key for authentication. `None` means authentication is disabled.
    pub api_key: Option<String>,
    /// Whether authentication is enforced.
    pub enabled: bool,
}

impl AuthConfig {
    /// Load authentication configuration from environment variables.
    ///
    /// Returns `Ok(AuthConfig)` when the configuration is valid:
    /// - `ROUTER_AUTH_ENABLED` is absent or `"false"` (auth disabled), or
    /// - `ROUTER_AUTH_ENABLED=true` **and** `ROUTER_API_KEY` is set (auth enabled).
    ///
    /// # Errors
    ///
    /// Returns `Err` if `ROUTER_AUTH_ENABLED=true` but `ROUTER_API_KEY` is not
    /// set. Silently degrading to an unauthenticated server in this case would
    /// create a dangerous misconfiguration that is easy to miss in production
    /// logs, so the server refuses to start instead.
    pub fn from_env() -> Result<Self> {
        let enabled = env::var("ROUTER_AUTH_ENABLED")
            .map(|v| v.to_lowercase() == "true")
            .unwrap_or(false);

        let api_key = env::var("ROUTER_API_KEY").ok();

        if enabled && api_key.is_none() {
            bail!(
                "ROUTER_AUTH_ENABLED=true but ROUTER_API_KEY is not set. \
                 Set ROUTER_API_KEY to a secret value or disable auth by \
                 setting ROUTER_AUTH_ENABLED=false."
            );
        }

        Ok(AuthConfig {
            enabled: enabled && api_key.is_some(),
            api_key,
        })
    }
}

/// Axum middleware that validates API keys.
///
/// When authentication is disabled (`config.enabled == false`) all requests
/// pass through unchanged.
pub async fn auth_middleware(
    axum::extract::State(config): axum::extract::State<AuthConfig>,
    req: Request,
    next: Next,
) -> Result<Response, AuthError> {
    if !config.enabled {
        return Ok(next.run(req).await);
    }

    let headers = req.headers();
    let api_key = extract_api_key(headers);

    match api_key {
        Some(key) => {
            if let Some(expected_key) = &config.api_key {
                if api_keys_equal(&key, expected_key) {
                    Ok(next.run(req).await)
                } else {
                    Err(AuthError::InvalidKey)
                }
            } else {
                Err(AuthError::Unauthorized)
            }
        }
        None => Err(AuthError::MissingKey),
    }
}

/// Compare an API key against the expected value in constant time.
///
/// A plain `==` on `String` short-circuits at the first mismatched byte, so
/// the comparison latency reveals how many leading bytes matched. An attacker
/// measuring response times could therefore recover `ROUTER_API_KEY`
/// byte-by-byte. [`ConstantTimeEq`] compares every byte and accumulates the
/// result via a bitwise OR, so the comparison time does not depend on where
/// (or whether) the first mismatch occurs.
fn api_keys_equal(supplied: &str, expected: &str) -> bool {
    supplied.as_bytes().ct_eq(expected.as_bytes()).into()
}

/// Extract the API key from request headers.
///
/// Checks `Authorization: Bearer <key>` first, then `X-API-Key: <key>`.
fn extract_api_key(headers: &HeaderMap) -> Option<String> {
    // Try Authorization: Bearer <key>
    if let Some(auth_header) = headers.get("authorization") {
        if let Ok(auth_str) = auth_header.to_str() {
            if let Some(key) = auth_str.strip_prefix("Bearer ") {
                return Some(key.to_string());
            }
        }
    }

    // Try X-API-Key: <key>
    if let Some(api_key_header) = headers.get("x-api-key") {
        if let Ok(key) = api_key_header.to_str() {
            return Some(key.to_string());
        }
    }

    None
}

/// Authentication errors returned by [`auth_middleware`].
#[derive(Debug)]
pub enum AuthError {
    /// No API key was included in the request.
    MissingKey,
    /// An API key was included but it did not match the expected value.
    InvalidKey,
    /// Catch-all unauthorised error (e.g. auth enabled but no key configured).
    Unauthorized,
}

impl IntoResponse for AuthError {
    fn into_response(self) -> Response {
        let (status, message) = match self {
            AuthError::MissingKey => (StatusCode::UNAUTHORIZED, "Missing API key"),
            AuthError::InvalidKey => (StatusCode::UNAUTHORIZED, "Invalid API key"),
            AuthError::Unauthorized => (StatusCode::FORBIDDEN, "Unauthorized"),
        };
        (status, message).into_response()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderMap;
    use std::sync::Mutex;

    // Guard serialises tests that mutate environment variables so they don't
    // interfere with each other when the test suite runs in parallel.
    static ENV_MUTEX: Mutex<()> = Mutex::new(());

    // ── AuthConfig::from_env tests ────────────────────────────────────

    #[test]
    fn test_from_env_auth_disabled_by_default() {
        let _guard = ENV_MUTEX.lock().unwrap();
        env::remove_var("ROUTER_AUTH_ENABLED");
        env::remove_var("ROUTER_API_KEY");

        let config = AuthConfig::from_env().expect("should succeed when auth is disabled");
        assert!(!config.enabled);
        assert!(config.api_key.is_none());
    }

    #[test]
    fn test_from_env_auth_enabled_with_key() {
        let _guard = ENV_MUTEX.lock().unwrap();
        env::set_var("ROUTER_AUTH_ENABLED", "true");
        env::set_var("ROUTER_API_KEY", "my-secret-key");

        let config = AuthConfig::from_env().expect("should succeed when auth is enabled and key is set");
        assert!(config.enabled);
        assert_eq!(config.api_key.as_deref(), Some("my-secret-key"));

        env::remove_var("ROUTER_AUTH_ENABLED");
        env::remove_var("ROUTER_API_KEY");
    }

    #[test]
    fn test_from_env_auth_enabled_without_key_fails() {
        let _guard = ENV_MUTEX.lock().unwrap();
        env::set_var("ROUTER_AUTH_ENABLED", "true");
        env::remove_var("ROUTER_API_KEY");

        let result = AuthConfig::from_env();
        assert!(
            result.is_err(),
            "should fail when ROUTER_AUTH_ENABLED=true but ROUTER_API_KEY is unset"
        );
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("ROUTER_AUTH_ENABLED=true") && err.contains("ROUTER_API_KEY"),
            "error message should mention both variables; got: {err}"
        );

        env::remove_var("ROUTER_AUTH_ENABLED");
    }

    #[test]
    fn test_from_env_auth_explicitly_disabled_no_key() {
        let _guard = ENV_MUTEX.lock().unwrap();
        env::set_var("ROUTER_AUTH_ENABLED", "false");
        env::remove_var("ROUTER_API_KEY");

        let config = AuthConfig::from_env()
            .expect("should succeed when auth is explicitly disabled");
        assert!(!config.enabled);

        env::remove_var("ROUTER_AUTH_ENABLED");
    }

    #[test]
    fn test_from_env_key_set_but_auth_disabled() {
        let _guard = ENV_MUTEX.lock().unwrap();
        env::remove_var("ROUTER_AUTH_ENABLED");
        env::set_var("ROUTER_API_KEY", "unused-key");

        // Key present but auth not explicitly enabled — auth stays off.
        let config = AuthConfig::from_env().expect("should succeed");
        assert!(!config.enabled);

        env::remove_var("ROUTER_API_KEY");
    }

    #[test]
    fn test_extract_bearer_token() {
        let mut headers = HeaderMap::new();
        headers.insert("authorization", "Bearer test-key-123".parse().unwrap());
        assert_eq!(extract_api_key(&headers), Some("test-key-123".to_string()));
    }

    #[test]
    fn test_extract_api_key_header() {
        let mut headers = HeaderMap::new();
        headers.insert("x-api-key", "test-key-456".parse().unwrap());
        assert_eq!(extract_api_key(&headers), Some("test-key-456".to_string()));
    }

    #[test]
    fn test_extract_api_key_missing() {
        let headers = HeaderMap::new();
        assert_eq!(extract_api_key(&headers), None);
    }

    #[test]
    fn test_bearer_token_takes_precedence() {
        let mut headers = HeaderMap::new();
        headers.insert("authorization", "Bearer bearer-key".parse().unwrap());
        headers.insert("x-api-key", "api-key".parse().unwrap());
        assert_eq!(extract_api_key(&headers), Some("bearer-key".to_string()));
    }

    // ── constant-time comparison tests ───────────────────────────────

    #[test]
    fn test_api_keys_equal_matching_keys() {
        assert!(api_keys_equal("secret-api-key-123", "secret-api-key-123"));
    }

    #[test]
    fn test_api_keys_equal_mismatch_at_start() {
        assert!(!api_keys_equal("aaaa", "bbbb"));
    }

    #[test]
    fn test_api_keys_equal_mismatch_at_end() {
        assert!(!api_keys_equal("abcX", "abcY"));
    }

    #[test]
    fn test_api_keys_equal_different_lengths() {
        assert!(!api_keys_equal("short", "shortlong"));
        assert!(!api_keys_equal("longlong", "long"));
    }

    #[test]
    fn test_api_keys_equal_empty_keys() {
        assert!(api_keys_equal("", ""));
        assert!(!api_keys_equal("", "not-empty"));
        assert!(!api_keys_equal("not-empty", ""));
    }
}

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
//! | `ROUTER_API_KEY` | — | API key for authentication. If unset, authentication is disabled. |
//! | `ROUTER_AUTH_ENABLED` | `false` | Set to `"true"` to require authentication. |

use axum::{
    extract::Request,
    http::{HeaderMap, StatusCode},
    middleware::Next,
    response::{IntoResponse, Response},
};
use std::env;
use tracing::warn;

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
    /// Authentication is only active when **both** `ROUTER_AUTH_ENABLED=true` and
    /// `ROUTER_API_KEY` is set. If auth is enabled but no key is configured a
    /// warning is emitted and authentication is silently disabled.
    pub fn from_env() -> Self {
        let enabled = env::var("ROUTER_AUTH_ENABLED")
            .map(|v| v.to_lowercase() == "true")
            .unwrap_or(false);

        let api_key = env::var("ROUTER_API_KEY").ok();

        if enabled && api_key.is_none() {
            warn!(
                "Authentication enabled but ROUTER_API_KEY not set. \
                 Authentication will be disabled."
            );
        }

        AuthConfig {
            enabled: enabled && api_key.is_some(),
            api_key,
        }
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
                if key == *expected_key {
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
}

//! Request validation for incoming HTTP query parameters.
//!
//! All public entry points return a structured [`ValidationError`] on failure
//! so callers can return a clear 400 response with a descriptive message.
//! No sensitive data is included in error messages.

use axum::{
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use serde::Serialize;

/// A structured validation error returned as JSON with HTTP 400.
#[derive(Debug, Serialize)]
pub struct ValidationError {
    pub error: &'static str,
    pub message: String,
}

impl IntoResponse for ValidationError {
    fn into_response(self) -> Response {
        (StatusCode::UNPROCESSABLE_ENTITY, Json(self)).into_response()
    }
}

impl ValidationError {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            error: "validation_error",
            message: message.into(),
        }
    }
}

// ── Validation rules ──────────────────────────────────────────────────────────

/// Validate a contract ID: must be a 56-character alphanumeric Stellar address.
pub fn validate_contract_id(id: &str) -> Result<(), ValidationError> {
    if id.is_empty() {
        return Err(ValidationError::new("contract_id must not be empty"));
    }
    if id.len() != 56 {
        return Err(ValidationError::new(
            "contract_id must be exactly 56 characters",
        ));
    }
    if !id.chars().all(|c| c.is_ascii_alphanumeric()) {
        return Err(ValidationError::new(
            "contract_id must contain only alphanumeric characters",
        ));
    }
    if !id.starts_with('C') {
        return Err(ValidationError::new("contract_id must start with 'C'"));
    }
    Ok(())
}

/// Validate a route name: non-empty, max 64 chars, alphanumeric + underscore/hyphen.
pub fn validate_route_name(name: &str) -> Result<(), ValidationError> {
    if name.is_empty() {
        return Err(ValidationError::new("route name must not be empty"));
    }
    if name.len() > 64 {
        return Err(ValidationError::new(
            "route name must be 64 characters or fewer",
        ));
    }
    if !name
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
    {
        return Err(ValidationError::new(
            "route name must contain only alphanumeric characters, underscores, or hyphens",
        ));
    }
    Ok(())
}

/// Validate a scrape interval: must be between 1 and 3600 seconds.
pub fn validate_scrape_interval(secs: u64) -> Result<(), ValidationError> {
    if secs == 0 {
        return Err(ValidationError::new(
            "scrape_interval_secs must be greater than 0",
        ));
    }
    if secs > 3600 {
        return Err(ValidationError::new(
            "scrape_interval_secs must not exceed 3600",
        ));
    }
    Ok(())
}

/// Validate the listen address: must be a valid `host:port` string.
pub fn validate_listen_addr(addr: &str) -> Result<(), ValidationError> {
    addr.parse::<std::net::SocketAddr>()
        .map(|_| ())
        .map_err(|_| ValidationError::new("listen address must be a valid host:port (e.g. 0.0.0.0:9090)"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn valid_contract_id() {
        let id = format!("C{}", "A".repeat(55));
        assert!(validate_contract_id(&id).is_ok());
    }

    #[test]
    fn short_contract_id_rejected() {
        assert!(validate_contract_id("SHORT").is_err());
    }

    #[test]
    fn empty_contract_id_rejected() {
        assert!(validate_contract_id("").is_err());
    }

    #[test]
    fn contract_id_with_special_chars_rejected() {
        let id = format!("{}!", "A".repeat(55));
        assert!(validate_contract_id(&id).is_err());
    }

    #[test]
    fn contract_id_with_g_prefix_rejected() {
        let id = "GAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAABSC4";
        assert!(validate_contract_id(id).is_err());
    }

    #[test]
    fn valid_route_name() {
        assert!(validate_route_name("oracle_feed-v2").is_ok());
    }

    #[test]
    fn empty_route_name_rejected() {
        assert!(validate_route_name("").is_err());
    }

    #[test]
    fn long_route_name_rejected() {
        assert!(validate_route_name(&"a".repeat(65)).is_err());
    }

    #[test]
    fn route_name_with_spaces_rejected() {
        assert!(validate_route_name("bad name").is_err());
    }

    #[test]
    fn valid_scrape_interval() {
        assert!(validate_scrape_interval(15).is_ok());
    }

    #[test]
    fn zero_scrape_interval_rejected() {
        assert!(validate_scrape_interval(0).is_err());
    }

    #[test]
    fn too_large_scrape_interval_rejected() {
        assert!(validate_scrape_interval(3601).is_err());
    }

    #[test]
    fn valid_listen_addr() {
        assert!(validate_listen_addr("0.0.0.0:9090").is_ok());
    }

    #[test]
    fn invalid_listen_addr_rejected() {
        assert!(validate_listen_addr("not-an-addr").is_err());
    }
}

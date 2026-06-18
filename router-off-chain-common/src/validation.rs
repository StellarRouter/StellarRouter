//! Input validation helpers for off-chain services.
//!
//! All public functions return [`ValidationError`] on failure so callers can
//! return a clear HTTP 422 response with a descriptive message. No sensitive
//! data is included in error messages.

use crate::error::ValidationError;

// ── Validation rules ──────────────────────────────────────────────────────────

/// Validate a Stellar contract ID.
///
/// A valid contract ID is exactly 56 alphanumeric ASCII characters.
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
    Ok(())
}

/// Validate a route name.
///
/// A valid route name is non-empty, at most 64 characters, and contains only
/// ASCII alphanumeric characters, underscores (`_`), or hyphens (`-`).
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

/// Validate a scrape interval in seconds.
///
/// Must be between 1 and 3600 (inclusive).
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

/// Validate a listen address string.
///
/// Must be a valid `host:port` value (e.g. `"0.0.0.0:9090"`).
pub fn validate_listen_addr(addr: &str) -> Result<(), ValidationError> {
    addr.parse::<std::net::SocketAddr>()
        .map(|_| ())
        .map_err(|_| {
            ValidationError::new(
                "listen address must be a valid host:port (e.g. 0.0.0.0:9090)",
            )
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── validate_contract_id ──────────────────────────────────────────────────

    #[test]
    fn valid_contract_id() {
        let id = "A".repeat(56);
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

    // ── validate_route_name ───────────────────────────────────────────────────

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

    // ── validate_scrape_interval ──────────────────────────────────────────────

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

    // ── validate_listen_addr ──────────────────────────────────────────────────

    #[test]
    fn valid_listen_addr() {
        assert!(validate_listen_addr("0.0.0.0:9090").is_ok());
    }

    #[test]
    fn invalid_listen_addr_rejected() {
        assert!(validate_listen_addr("not-an-addr").is_err());
    }
}

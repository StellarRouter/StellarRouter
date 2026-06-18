//! Replay attack protection middleware for the metrics exporter.
//!
//! Prevents duplicate or malicious repeated transaction submissions using nonce-based approach.
//!
//! Configuration via environment variables:
//! - `ROUTER_REPLAY_PROTECTION_ENABLED` — Set to "true" to enable replay protection (default: false)
//! - `ROUTER_NONCE_CACHE_SIZE` — Maximum number of nonces to cache (default: 10000)
//! - `ROUTER_NONCE_TTL_SECS` — Time-to-live for nonces in seconds (default: 3600)

use axum::{
    extract::Request,
    http::{HeaderMap, StatusCode},
    middleware::Next,
    response::{IntoResponse, Response},
};
use dashmap::DashMap;
use std::env;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use tracing::{debug, warn};

/// Replay protection configuration.
#[derive(Clone, Debug)]
pub struct ReplayProtectionConfig {
    /// Whether replay protection is enabled.
    pub enabled: bool,
    /// Maximum number of nonces to cache.
    pub cache_size: usize,
    /// Time-to-live for nonces in seconds.
    pub nonce_ttl_secs: u64,
}

impl ReplayProtectionConfig {
    /// Load replay protection configuration from environment variables.
    pub fn from_env() -> Self {
        let enabled = env::var("ROUTER_REPLAY_PROTECTION_ENABLED")
            .map(|v| v.to_lowercase() == "true")
            .unwrap_or(false);

        let cache_size = env::var("ROUTER_NONCE_CACHE_SIZE")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(10000);

        let nonce_ttl_secs = env::var("ROUTER_NONCE_TTL_SECS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(3600);

        ReplayProtectionConfig {
            enabled,
            cache_size,
            nonce_ttl_secs,
        }
    }
}

/// Nonce cache entry with timestamp.
#[derive(Clone, Debug)]
struct NonceEntry {
    timestamp: u64,
}

/// Nonce cache for replay attack detection.
#[derive(Clone)]
pub struct NonceCache {
    cache: Arc<DashMap<String, NonceEntry>>,
    config: ReplayProtectionConfig,
}

impl NonceCache {
    /// Create a new nonce cache.
    pub fn new(config: ReplayProtectionConfig) -> Self {
        NonceCache {
            cache: Arc::new(DashMap::new()),
            config,
        }
    }

    /// Check if a nonce has been seen before and add it to the cache.
    /// Returns true if the nonce is valid (not seen before), false if it's a replay.
   pub fn check_and_add(&self, nonce: &str) -> bool {
        let now = current_timestamp();

        // Clean up expired nonces
        self.cleanup_expired(now);

        // Check if nonce already exists
        if self.cache.contains_key(nonce) {
            debug!("Replay attack detected: nonce {} already seen", nonce);
            return false;
        }

        // Check cache size limit
        if self.cache.len() >= self.config.cache_size {
            warn!(
                "Nonce cache at capacity ({}), rejecting new nonce",
                self.config.cache_size
            );
            return false;
        }

        // Add nonce to cache using the proper NonceEntry struct wrapper
        self.cache.insert(
            nonce.to_string(),
            NonceEntry { timestamp: now },
        );
        true
    }


    /// Clean up expired nonces from the cache.
    fn cleanup_expired(&self, now: u64) {
        let ttl = self.config.nonce_ttl_secs;
        self.cache.retain(|_, entry| now - entry.timestamp < ttl);
    }
}

/// Extract nonce from request headers.
fn extract_nonce(headers: &HeaderMap) -> Option<String> {
    headers
        .get("x-nonce")
        .and_then(|h| h.to_str().ok())
        .map(|s| s.to_string())
}

/// Get current Unix timestamp in seconds.
fn current_timestamp() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Replay attack protection error.
#[derive(Debug)]
pub enum ReplayError {
    /// Nonce is missing from request.
    MissingNonce,
    /// Nonce has been seen before (replay attack detected).
    DuplicateNonce,
}

impl IntoResponse for ReplayError {
    fn into_response(self) -> Response {
        let (status, message) = match self {
            ReplayError::MissingNonce => (StatusCode::BAD_REQUEST, "Missing X-Nonce header"),
            ReplayError::DuplicateNonce => (
                StatusCode::CONFLICT,
                "Duplicate nonce detected (replay attack)",
            ),
        };

        (status, message).into_response()
    }
}

use axum::extract::State;

/// Replay attack protection middleware.
pub async fn replay_protection_middleware(
    State(cache): State<NonceCache>,
    req: Request,
    next: Next,
) -> Response {
    // Skip protection if disabled
    if !cache.config.enabled {
        return next.run(req).await;
    }

    let headers = req.headers();
    let nonce = match extract_nonce(headers) {
        Some(n) => n,
        None => return ReplayError::MissingNonce.into_response(),
    };

    if cache.check_and_add(&nonce) {
        next.run(req).await
    } else {
        ReplayError::DuplicateNonce.into_response()
    }
}
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_nonce_cache_accepts_new_nonce() {
        let config = ReplayProtectionConfig {
            enabled: true,
            cache_size: 100,
            nonce_ttl_secs: 3600,
        };
        let cache = NonceCache::new(config);

        assert!(cache.check_and_add("nonce-1"));
    }

    #[test]
    fn test_nonce_cache_rejects_duplicate() {
        let config = ReplayProtectionConfig {
            enabled: true,
            cache_size: 100,
            nonce_ttl_secs: 3600,
        };
        let cache = NonceCache::new(config);

        assert!(cache.check_and_add("nonce-1"));
        assert!(!cache.check_and_add("nonce-1"));
    }

    #[test]
    fn test_nonce_cache_respects_size_limit() {
        let config = ReplayProtectionConfig {
            enabled: true,
            cache_size: 2,
            nonce_ttl_secs: 3600,
        };
        let cache = NonceCache::new(config);

        assert!(cache.check_and_add("nonce-1"));
        assert!(cache.check_and_add("nonce-2"));
        assert!(!cache.check_and_add("nonce-3")); // Cache full
    }

    #[test]
    fn test_extract_nonce() {
        let mut headers = HeaderMap::new();
        headers.insert("x-nonce", "test-nonce-123".parse().unwrap());

        let nonce = extract_nonce(&headers);
        assert_eq!(nonce, Some("test-nonce-123".to_string()));
    }
}


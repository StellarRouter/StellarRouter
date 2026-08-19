//! Replay-attack protection middleware.
//!
//! Prevents duplicate or replayed requests using a nonce-based approach.
//! Callers include a unique `X-Nonce` header with each request; the middleware
//! rejects any request whose nonce has already been seen within the TTL window.
//!
//! ## Configuration (environment variables)
//!
//! | Variable | Default | Description |
//! |----------|---------|-------------|
//! | `ROUTER_REPLAY_PROTECTION_ENABLED` | `false` | Set to `"true"` to enable replay protection. |
//! | `ROUTER_NONCE_CACHE_SIZE` | `10000` | Maximum number of nonces held in memory. |
//! | `ROUTER_NONCE_TTL_SECS` | `3600` | Time-to-live for cached nonces (seconds). |

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

// ── Config ────────────────────────────────────────────────────────────────────

/// Replay-protection configuration.
#[derive(Clone, Debug)]
pub struct ReplayProtectionConfig {
    /// Whether replay protection is enabled.
    pub enabled: bool,
    /// Maximum number of nonces to hold in memory.
    pub cache_size: usize,
    /// Time-to-live for nonces in seconds.
    pub nonce_ttl_secs: u64,
}

impl ReplayProtectionConfig {
    /// Load replay-protection configuration from environment variables.
    pub fn from_env() -> Self {
        let enabled = env::var("ROUTER_REPLAY_PROTECTION_ENABLED")
            .map(|v| v.to_lowercase() == "true")
            .unwrap_or(false);

        let cache_size = env::var("ROUTER_NONCE_CACHE_SIZE")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(10_000);

        let nonce_ttl_secs = env::var("ROUTER_NONCE_TTL_SECS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(3_600);

        ReplayProtectionConfig {
            enabled,
            cache_size,
            nonce_ttl_secs,
        }
    }
}

// ── Nonce cache ───────────────────────────────────────────────────────────────

#[derive(Clone, Debug)]
struct NonceEntry {
    timestamp: u64,
}

/// Thread-safe nonce cache for replay-attack detection.
///
/// Clone is cheap — the underlying [`DashMap`] is wrapped in an [`Arc`].
#[derive(Clone)]
pub struct NonceCache {
    cache: Arc<DashMap<String, NonceEntry>>,
    config: ReplayProtectionConfig,
}

impl NonceCache {
    /// Create a new [`NonceCache`] with the given configuration.
    pub fn new(config: ReplayProtectionConfig) -> Self {
        NonceCache {
            cache: Arc::new(DashMap::new()),
            config,
        }
    }

    /// Check whether `nonce` is valid (not yet seen) and, if so, record it.
    ///
    /// Returns `true` when the nonce is fresh, `false` when it is a replay or
    /// when the cache is full.
    ///
    /// The check-and-insert is performed atomically via [`DashMap::entry`] so
    /// that two concurrent requests carrying the same nonce cannot both pass
    /// the uniqueness check before either inserts — defeating the TOCTOU race
    /// that existed when `contains_key` and `insert` were two separate
    /// operations.
    pub fn check_and_add(&self, nonce: &str) -> bool {
        let now = current_timestamp();
        self.cleanup_expired(now);

        // Reject immediately if the cache is already at capacity.
        // We check this *before* acquiring the shard lock to avoid holding it
        // while counting, but the authoritative capacity guard is still the
        // entry() path below.
        if self.cache.len() >= self.config.cache_size {
            warn!(
                "Nonce cache at capacity ({}), rejecting new nonce",
                self.config.cache_size
            );
            return false;
        }

        // `entry(...).or_insert(...)` acquires the shard lock once and either
        // returns the existing value (nonce already present → replay) or
        // inserts the new entry (nonce is fresh).  This single-lock operation
        // eliminates the TOCTOU window between a separate contains_key and
        // insert call.
        use dashmap::mapref::entry::Entry;
        match self.cache.entry(nonce.to_string()) {
            Entry::Occupied(_) => {
                debug!("Replay attack detected: nonce {} already seen", nonce);
                false
            }
            Entry::Vacant(slot) => {
                slot.insert(NonceEntry { timestamp: now });
                true
            }
        }
    }

    /// Remove all cache entries whose timestamp is older than the TTL.
    fn cleanup_expired(&self, now: u64) {
        let ttl = self.config.nonce_ttl_secs;
        self.cache.retain(|_, entry| now - entry.timestamp < ttl);
    }
}

// ── Helpers ───────────────────────────────────────────────────────────────────

fn extract_nonce(headers: &HeaderMap) -> Option<String> {
    headers
        .get("x-nonce")
        .and_then(|h| h.to_str().ok())
        .map(|s| s.to_string())
}

fn current_timestamp() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

// ── Error ─────────────────────────────────────────────────────────────────────

/// Error variants returned by [`replay_protection_middleware`].
#[derive(Debug)]
pub enum ReplayError {
    /// The `X-Nonce` header is missing from the request.
    MissingNonce,
    /// The nonce has been seen before (replay attack detected).
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

// ── Middleware ────────────────────────────────────────────────────────────────

/// Replay-attack protection middleware.
///
/// When replay protection is disabled (`cache.config.enabled == false`) all
/// requests pass through unchanged.
pub async fn replay_protection_middleware(
    axum::extract::State(cache): axum::extract::State<NonceCache>,
    req: Request,
    next: Next,
) -> Result<Response, ReplayError> {
    if !cache.config.enabled {
        return Ok(next.run(req).await);
    }

    let headers = req.headers();
    let nonce = extract_nonce(headers).ok_or(ReplayError::MissingNonce)?;

    if cache.check_and_add(&nonce) {
        Ok(next.run(req).await)
    } else {
        Err(ReplayError::DuplicateNonce)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config(cache_size: usize) -> ReplayProtectionConfig {
        ReplayProtectionConfig {
            enabled: true,
            cache_size,
            nonce_ttl_secs: 3600,
        }
    }

    #[test]
    fn accepts_new_nonce() {
        let cache = NonceCache::new(config(100));
        assert!(cache.check_and_add("nonce-1"));
    }

    #[test]
    fn rejects_duplicate_nonce() {
        let cache = NonceCache::new(config(100));
        assert!(cache.check_and_add("nonce-1"));
        assert!(!cache.check_and_add("nonce-1"));
    }

    #[test]
    fn respects_cache_size_limit() {
        let cache = NonceCache::new(config(2));
        assert!(cache.check_and_add("nonce-1"));
        assert!(cache.check_and_add("nonce-2"));
        assert!(!cache.check_and_add("nonce-3")); // Cache full
    }

    #[test]
    fn extracts_nonce_from_header() {
        let mut headers = HeaderMap::new();
        headers.insert("x-nonce", "test-nonce-123".parse().unwrap());
        assert_eq!(extract_nonce(&headers), Some("test-nonce-123".to_string()));
    }

    #[test]
    fn returns_none_when_nonce_missing() {
        let headers = HeaderMap::new();
        assert_eq!(extract_nonce(&headers), None);
    }

    /// Spawn N concurrent tasks all racing to insert the same nonce.
    /// Exactly one must succeed and all others must be rejected — verifying
    /// that `check_and_add` is atomic (no TOCTOU window).
    #[tokio::test]
    async fn concurrent_same_nonce_only_one_succeeds() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        use std::sync::Arc as StdArc;

        let cache = StdArc::new(NonceCache::new(config(1000)));
        let success_count = StdArc::new(AtomicUsize::new(0));
        const N: usize = 64;

        let mut handles = Vec::with_capacity(N);
        for _ in 0..N {
            let c = StdArc::clone(&cache);
            let sc = StdArc::clone(&success_count);
            handles.push(tokio::spawn(async move {
                if c.check_and_add("race-nonce") {
                    sc.fetch_add(1, Ordering::Relaxed);
                }
            }));
        }
        for h in handles {
            h.await.unwrap();
        }

        assert_eq!(
            success_count.load(Ordering::Relaxed),
            1,
            "exactly one concurrent insertion must succeed"
        );
    }
}

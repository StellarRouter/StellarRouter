//! Token-bucket rate-limiting middleware.
//!
//! Limits requests per remote IP address or, when present, per `X-API-Key`
//! header value. The bucket resets after each [`RateLimitConfig::window`]
//! duration.
//!
//! Returns HTTP 429 with a JSON body and a `Retry-After` header when the
//! limit is exceeded.
//!
//! ## Configuration (environment variables)
//!
//! | Variable | Default | Description |
//! |----------|---------|-------------|
//! | `ROUTER_RATE_LIMIT_MAX_REQUESTS` | `60` | Maximum requests allowed per window. |
//! | `ROUTER_RATE_LIMIT_WINDOW_SECS` | `60` | Window duration in seconds. |

use std::{
    net::SocketAddr,
    sync::Arc,
    time::{Duration, Instant},
};

use axum::{
    body::Body,
    extract::{ConnectInfo, State},
    http::{Request, StatusCode},
    middleware::Next,
    response::{IntoResponse, Json, Response},
};
use dashmap::DashMap;
use serde::Serialize;
use tracing::warn;

// ── Config ────────────────────────────────────────────────────────────────────

/// Rate-limit configuration.
#[derive(Debug, Clone)]
pub struct RateLimitConfig {
    /// Maximum requests allowed per window.
    pub max_requests: u32,
    /// Duration of the sliding window.
    pub window: Duration,
}

impl Default for RateLimitConfig {
    fn default() -> Self {
        Self {
            max_requests: 60,
            window: Duration::from_secs(60),
        }
    }
}

impl RateLimitConfig {
    /// Load rate-limit settings from environment variables, falling back to
    /// defaults when the variables are unset.
    ///
    /// Returns an error if a variable is present but cannot be parsed.
    pub fn from_env() -> anyhow::Result<Self> {
        let max_requests = parse_optional_u32("ROUTER_RATE_LIMIT_MAX_REQUESTS")?
            .unwrap_or(60);
        if max_requests == 0 {
            return Err(anyhow::anyhow!(
                "ROUTER_RATE_LIMIT_MAX_REQUESTS must be greater than 0"
            ));
        }

        let window_secs = parse_optional_u64("ROUTER_RATE_LIMIT_WINDOW_SECS")?
            .unwrap_or(60);
        if window_secs == 0 {
            return Err(anyhow::anyhow!(
                "ROUTER_RATE_LIMIT_WINDOW_SECS must be greater than 0"
            ));
        }

        Ok(Self {
            max_requests,
            window: Duration::from_secs(window_secs),
        })
    }
}

fn parse_optional_u32(name: &str) -> anyhow::Result<Option<u32>> {
    match std::env::var(name) {
        Ok(value) => value
            .parse::<u32>()
            .map(Some)
            .map_err(|_| anyhow::anyhow!("{name} must be a positive integer")),
        Err(std::env::VarError::NotPresent) => Ok(None),
        Err(e) => Err(anyhow::anyhow!("failed to read {name}: {e}")),
    }
}

fn parse_optional_u64(name: &str) -> anyhow::Result<Option<u64>> {
    match std::env::var(name) {
        Ok(value) => value
            .parse::<u64>()
            .map(Some)
            .map_err(|_| anyhow::anyhow!("{name} must be a positive integer")),
        Err(std::env::VarError::NotPresent) => Ok(None),
        Err(e) => Err(anyhow::anyhow!("failed to read {name}: {e}")),
    }
}

// ── State ─────────────────────────────────────────────────────────────────────

#[derive(Debug)]
struct BucketEntry {
    count: u32,
    window_start: Instant,
}

/// Shared token-bucket limiter state. Cheap to clone (`Arc` inside).
#[derive(Clone, Debug)]
pub struct RateLimiter {
    config: RateLimitConfig,
    buckets: Arc<DashMap<String, BucketEntry>>,
}

impl RateLimiter {
    /// Create a new [`RateLimiter`] with the given configuration.
    pub fn new(config: RateLimitConfig) -> Self {
        Self {
            config,
            buckets: Arc::new(DashMap::new()),
        }
    }

    /// Returns `true` if the request identified by `key` is within the
    /// configured limit, `false` if it should be rejected.
    pub fn check(&self, key: &str) -> bool {
        let now = Instant::now();
        let mut entry = self.buckets.entry(key.to_string()).or_insert(BucketEntry {
            count: 0,
            window_start: now,
        });

        // Reset window if it has expired.
        if now.duration_since(entry.window_start) >= self.config.window {
            entry.count = 0;
            entry.window_start = now;
        }

        entry.count += 1;
        entry.count <= self.config.max_requests
    }

    /// Returns how many seconds remain before the current window expires for
    /// `key`, floored to 1.
    pub fn retry_after_secs(&self, key: &str) -> u64 {
        if let Some(entry) = self.buckets.get(key) {
            let elapsed = Instant::now().duration_since(entry.window_start);
            if elapsed < self.config.window {
                return (self.config.window - elapsed).as_secs().max(1);
            }
        }
        1
    }
}

// ── Middleware ────────────────────────────────────────────────────────────────

#[derive(Serialize)]
struct RateLimitErrorBody {
    error: &'static str,
    message: String,
    retry_after_secs: u64,
}

/// Axum middleware that enforces per-IP (or per-API-key) rate limits.
///
/// The rate-limit key is the value of the `X-API-Key` header when present,
/// otherwise the remote IP address.
pub async fn rate_limit_middleware(
    State(limiter): State<RateLimiter>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    req: Request<Body>,
    next: Next,
) -> Response {
    // Prefer X-Api-Key as the rate-limit key; fall back to remote IP.
    let key = req
        .headers()
        .get("x-api-key")
        .and_then(|v| v.to_str().ok())
        .filter(|v| !v.is_empty())
        .map(|v| format!("api-key:{v}"))
        .unwrap_or_else(|| format!("ip:{}", addr.ip()));

    if limiter.check(&key) {
        return next.run(req).await;
    }

    let retry_after = limiter.retry_after_secs(&key);
    warn!(key = %key, "rate limit exceeded");

    (
        StatusCode::TOO_MANY_REQUESTS,
        [("retry-after", retry_after.to_string())],
        Json(RateLimitErrorBody {
            error: "rate_limit_exceeded",
            message: format!("Too many requests. Retry after {retry_after} second(s)."),
            retry_after_secs: retry_after,
        }),
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn limiter(max: u32, window_secs: u64) -> RateLimiter {
        RateLimiter::new(RateLimitConfig {
            max_requests: max,
            window: Duration::from_secs(window_secs),
        })
    }

    #[test]
    fn allows_requests_within_limit() {
        let rl = limiter(3, 60);
        assert!(rl.check("127.0.0.1"));
        assert!(rl.check("127.0.0.1"));
        assert!(rl.check("127.0.0.1"));
    }

    #[test]
    fn rejects_request_over_limit() {
        let rl = limiter(2, 60);
        rl.check("10.0.0.1");
        rl.check("10.0.0.1");
        assert!(!rl.check("10.0.0.1"));
    }

    #[test]
    fn different_keys_are_independent() {
        let rl = limiter(1, 60);
        assert!(rl.check("192.168.1.1"));
        assert!(rl.check("192.168.1.2"));
        assert!(!rl.check("192.168.1.1"));
    }
}

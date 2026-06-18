//! Token-bucket rate limiting for protected API routes.

use std::{
    env::VarError,
    net::SocketAddr,
    sync::Arc,
    time::{Duration, Instant},
};

use anyhow::{anyhow, Context, Result};
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

const DEFAULT_MAX_REQUESTS: u32 = 60;
const DEFAULT_WINDOW_SECS: u64 = 60;

/// Rate limit settings for API requests.
#[derive(Debug, Clone, Copy)]
pub struct RateLimitConfig {
    pub max_requests: u32,
    pub window: Duration,
}

impl Default for RateLimitConfig {
    fn default() -> Self {
        Self {
            max_requests: DEFAULT_MAX_REQUESTS,
            window: Duration::from_secs(DEFAULT_WINDOW_SECS),
        }
    }
}

impl RateLimitConfig {
    /// Loads rate limit settings from `ROUTER_API_MAX_REQUESTS` and
    /// `ROUTER_API_RATE_WINDOW_SECS`, falling back to local-development
    /// defaults when the variables are unset.
    pub fn from_env() -> Result<Self> {
        let max_requests =
            parse_optional_u32("ROUTER_API_MAX_REQUESTS")?.unwrap_or(DEFAULT_MAX_REQUESTS);
        if max_requests == 0 {
            return Err(anyhow!("ROUTER_API_MAX_REQUESTS must be greater than 0"));
        }

        let window_secs =
            parse_optional_u64("ROUTER_API_RATE_WINDOW_SECS")?.unwrap_or(DEFAULT_WINDOW_SECS);
        if window_secs == 0 {
            return Err(anyhow!("ROUTER_API_RATE_WINDOW_SECS must be greater than 0"));
        }

        Ok(Self {
            max_requests,
            window: Duration::from_secs(window_secs),
        })
    }
}

fn parse_optional_u32(name: &str) -> Result<Option<u32>> {
    match std::env::var(name) {
        Ok(value) => value
            .parse::<u32>()
            .with_context(|| format!("{name} must be a positive integer"))
            .map(Some),
        Err(VarError::NotPresent) => Ok(None),
        Err(err) => Err(err).with_context(|| format!("failed to read {name}")),
    }
}

fn parse_optional_u64(name: &str) -> Result<Option<u64>> {
    match std::env::var(name) {
        Ok(value) => value
            .parse::<u64>()
            .with_context(|| format!("{name} must be a positive integer"))
            .map(Some),
        Err(VarError::NotPresent) => Ok(None),
        Err(err) => Err(err).with_context(|| format!("failed to read {name}")),
    }
}

#[derive(Debug)]
struct BucketEntry {
    count: u32,
    window_start: Instant,
}

/// Shared token-bucket limiter state.
#[derive(Clone, Debug)]
pub struct RateLimiter {
    config: RateLimitConfig,
    buckets: Arc<DashMap<String, BucketEntry>>,
}

impl RateLimiter {
    pub fn new(config: RateLimitConfig) -> Self {
        Self {
            config,
            buckets: Arc::new(DashMap::new()),
        }
    }

    pub fn check(&self, key: &str) -> bool {
        let now = Instant::now();
        let mut entry = self
            .buckets
            .entry(key.to_string())
            .or_insert(BucketEntry {
                count: 0,
                window_start: now,
            });

        if now.duration_since(entry.window_start) >= self.config.window {
            entry.count = 0;
            entry.window_start = now;
        }

        entry.count += 1;
        entry.count <= self.config.max_requests
    }

    pub fn retry_after_secs(&self, key: &str) -> u64 {
        self.buckets
            .get(key)
            .and_then(|entry| {
                let elapsed = Instant::now().duration_since(entry.window_start);
                (elapsed < self.config.window)
                    .then(|| (self.config.window - elapsed).as_secs().max(1))
            })
            .unwrap_or(1)
    }
}

#[derive(Serialize)]
struct RateLimitError {
    error: &'static str,
    message: String,
    retry_after_secs: u64,
}

/// Axum middleware enforcing per-API-key or per-IP request limits.
pub async fn rate_limit_middleware(
    State(limiter): State<RateLimiter>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    req: Request<Body>,
    next: Next,
) -> Response {
    let key = req
        .headers()
        .get("x-api-key")
        .and_then(|value| value.to_str().ok())
        .filter(|value| !value.is_empty())
        .map(|value| format!("api-key:{value}"))
        .unwrap_or_else(|| format!("ip:{}", addr.ip()));

    if limiter.check(&key) {
        return next.run(req).await;
    }

    let retry_after = limiter.retry_after_secs(&key);
    warn!(key = %key, "rate limit exceeded");

    (
        StatusCode::TOO_MANY_REQUESTS,
        [("retry-after", retry_after.to_string())],
        Json(RateLimitError {
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

    fn limiter(max_requests: u32, window_secs: u64) -> RateLimiter {
        RateLimiter::new(RateLimitConfig {
            max_requests,
            window: Duration::from_secs(window_secs),
        })
    }

    #[test]
    fn allows_requests_within_limit() {
        let limiter = limiter(2, 60);

        assert!(limiter.check("ip:127.0.0.1"));
        assert!(limiter.check("ip:127.0.0.1"));
    }

    #[test]
    fn rejects_requests_after_limit() {
        let limiter = limiter(1, 60);

        assert!(limiter.check("ip:127.0.0.1"));
        assert!(!limiter.check("ip:127.0.0.1"));
    }

    #[test]
    fn tracks_keys_independently() {
        let limiter = limiter(1, 60);

        assert!(limiter.check("ip:127.0.0.1"));
        assert!(limiter.check("ip:127.0.0.2"));
        assert!(!limiter.check("ip:127.0.0.1"));
    }
}

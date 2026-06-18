//! Re-exports the shared rate-limiting middleware from `router-off-chain-common`.
//!
//! See [`router_off_chain_common::rate_limit`] for full documentation.

pub use router_off_chain_common::rate_limit::{
    rate_limit_middleware, RateLimitConfig, RateLimiter,
};

/// Load rate-limit settings from environment variables using the shared config.
///
/// This is a convenience wrapper kept for backwards compatibility with the
/// existing metrics binary.
pub fn config_from_env() -> RateLimitConfig {
    RateLimitConfig::from_env().unwrap_or_default()
}

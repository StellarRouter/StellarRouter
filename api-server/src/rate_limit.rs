//! Re-exports the shared rate-limiting middleware from `router-off-chain-common`.
//!
//! See [`router_off_chain_common::rate_limit`] for full documentation.

pub use router_off_chain_common::rate_limit::{
    rate_limit_middleware, RateLimitConfig, RateLimiter,
};

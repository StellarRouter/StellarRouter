//! # router-off-chain-common
//!
//! Shared off-chain utilities for the stellar-router suite.
//!
//! Both the API server (`router-api-server`) and the metrics exporter
//! (`router-metrics-exporter`) depend on this crate rather than maintaining
//! independent copies of the same middleware and utility code.
//!
//! ## Modules
//!
//! | Module | Contents |
//! |--------|----------|
//! | [`auth`] | `AuthConfig`, `auth_middleware`, `AuthError` — API-key authentication |
//! | [`rate_limit`] | `RateLimiter`, `RateLimitConfig`, `rate_limit_middleware` — token-bucket rate limiting |
//! | [`replay_protection`] | `NonceCache`, `ReplayProtectionConfig`, `replay_protection_middleware` — nonce-based replay attack prevention |
//! | [`validation`] | `validate_contract_id`, `validate_route_name`, etc. — input validation helpers |
//! | [`error`] | `ErrorResponse`, `ValidationError` — shared HTTP error response types |
//! | [`logging`] | `init_logging`, `new_request_id` — structured JSON logging setup |

pub mod auth;
pub mod error;
pub mod logging;
pub mod rate_limit;
pub mod replay_protection;
pub mod validation;

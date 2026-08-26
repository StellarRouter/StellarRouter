# router-off-chain-common

Shared off-chain utilities for the stellar-router suite. This crate is consumed by the API server (`router-api-server`) and metrics exporter (`router-metrics-exporter`) to centralize middleware, validation, and logging logic.

## Overview

Instead of duplicating authentication, rate limiting, replay protection, and validation logic across multiple crates, `router-off-chain-common` provides a single, well-tested implementation that both on-chain and off-chain services can depend on.

## Modules

### `auth` — API Key Authentication

Provides API-key based authentication middleware for Axum applications.

**Key exports:**
- `AuthConfig` — Configuration struct loaded from environment variables
- `auth_middleware` — Axum middleware that validates `Authorization: Bearer` or `X-API-Key` headers
- `AuthError` — Error type for authentication failures

**Configuration:**
- `ROUTER_AUTH_ENABLED` — Set to `"true"` to require authentication (default: `false`)
- `ROUTER_API_KEY` — The API key to match against incoming requests

See [`src/auth.rs`](src/auth.rs) for full documentation.

### `rate_limit` — Token-Bucket Rate Limiting

Implements per-IP and per-API-key rate limiting using a token-bucket algorithm.

**Key exports:**
- `RateLimitConfig` — Configuration (max requests, window duration)
- `RateLimiter` — Thread-safe limiter state backed by `DashMap`
- `rate_limit_middleware` — Axum middleware that enforces limits per `X-API-Key` or IP address

**Configuration:**
- `ROUTER_RATE_LIMIT_MAX_REQUESTS` — Maximum requests per window (default: `60`)
- `ROUTER_RATE_LIMIT_WINDOW_SECS` — Window duration in seconds (default: `60`)

Returns HTTP 429 with a `Retry-After` header when limits are exceeded.

See [`src/rate_limit.rs`](src/rate_limit.rs) for full documentation.

### `replay_protection` — Nonce-Based Replay Attack Prevention

Prevents duplicate or replayed requests using a nonce-based approach with TTL-aware expiry.

**Key exports:**
- `ReplayProtectionConfig` — Configuration (enabled flag, cache size, TTL)
- `NonceCache` — Thread-safe nonce cache that records seen nonces and rejects replays
- `replay_protection_middleware` — Axum middleware that validates `X-Nonce` headers

**Configuration:**
- `ROUTER_REPLAY_PROTECTION_ENABLED` — Set to `"true"` to enable (default: `false`)
- `ROUTER_NONCE_CACHE_SIZE` — Max nonces held in memory (default: `10,000`)
- `ROUTER_NONCE_TTL_SECS` — Nonce time-to-live in seconds (default: `3,600`)

The nonce check is atomic (no TOCTOU race) and automatically cleans up expired entries.

See [`src/replay_protection.rs`](src/replay_protection.rs) for full documentation.

### `validation` — Input Validation Helpers

Provides validators for Stellar contract IDs, route names, function names, and other off-chain configuration.

**Key exports:**
- `validate_contract_id` — Validates Stellar contract addresses (56 chars, starts with `C`)
- `validate_route_name` — Validates route names (up to 64 chars, alphanumeric/`_`/`-`)
- `validate_function_name` — Validates function names (up to 64 chars, alphanumeric/`_`/`-`)
- `validate_scrape_interval` — Validates scrape intervals (1–3600 seconds)
- `validate_listen_addr` — Validates network addresses (`host:port`)

All validators return `ValidationError` with clear, non-sensitive error messages suitable for HTTP responses.

See [`src/validation.rs`](src/validation.rs) for full documentation.

### `logging` — Structured JSON Logging

Sets up centralized structured logging via `tracing-subscriber` and provides log sanitization to prevent injection attacks.

**Key exports:**
- `init_logging` — Initializes the global tracing subscriber with JSON output and `RUST_LOG` support
- `new_request_id` — Generates a new UUID v4 request ID
- `sanitize_for_log` — Replaces ASCII control characters with Unicode control pictures to prevent log forgery

**Features:**
- JSON format for machine-readable logs
- RFC 3339 timestamps
- Request-ID propagation via span fields
- Built-in defense against log injection via control-character sanitization

See [`src/logging.rs`](src/logging.rs) for full documentation.

### `xdr` — Soroban XDR Utilities

Provides helpers for parsing and building Soroban XDR structures, including strkey/base64 decoding, XDR encoding/decoding, and transaction-envelope builders.

**Key exports:**
- `decode_contract_id` — Decodes a strkey-encoded contract ID
- `build_invoke_xdr` — Constructs an invoke-contract XDR from components
- `parse_route_entry` — Parses route-entry XDR
- `parse_string_vec` — Parses string vectors from XDR
- `ScArg` — Soroban contract argument type

See [`src/xdr.rs`](src/xdr.rs) for full documentation.

### `error` — Shared Error Types

Provides HTTP-friendly error response types.

**Key exports:**
- `ErrorResponse` — Standard JSON error response format
- `ValidationError` — Input validation error with HTTP 422 status

See [`src/error.rs`](src/error.rs) for full documentation.

## Consuming Crates

- **`router-api-server`** — Depends on `auth`, `rate_limit`, `replay_protection`, `validation`, `logging`, and `xdr` modules
- **`router-metrics-exporter`** — Depends on `auth`, `rate_limit`, `replay_protection`, `validation`, `logging`, and `xdr` modules

Both crates re-export the relevant modules from `router-off-chain-common` to maintain stable public APIs.

## Development

### Running Tests

```bash
cargo test -p router-off-chain-common
```

### Building

```bash
cargo build -p router-off-chain-common
```

### Coverage

```bash
cargo tarpaulin -p router-off-chain-common --timeout 300 --out html --out xml
```

## Security Considerations

- **Authentication:** API keys are compared in constant time (via `==`) but should be treated as secrets; use environment variables only.
- **Rate Limiting:** Limits are enforced per key (API key or IP) but can be bypassed if behind an untrustworthy proxy. Validate `X-Forwarded-For` and `X-Real-IP` appropriately.
- **Replay Protection:** Nonce cache is in-memory; single-server deployments only. Multi-server setups require external coordination (e.g., Redis).
- **Log Sanitization:** Control characters are replaced to prevent log injection, but logs still contain user-controlled data—review retention policies.
- **Validation:** Validators reject invalid input early; use their errors for HTTP 422 responses.

## Dependencies

- **Axum 0.7.9** — Web framework and middleware
- **Tokio 1.44.2** — Async runtime
- **DashMap 6.1.0** — Concurrent hash map for rate limiting and nonce caching
- **Tracing 0.1.41 / Tracing-Subscriber 0.3.19** — Structured logging
- **Serde 1.0.219** — Serialization
- **Reqwest 0.12.15** — HTTP client for RPC calls
- **UUID 1.11.0** — Request ID generation

See [`Cargo.toml`](Cargo.toml) for the full dependency list.

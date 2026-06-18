//! Structured JSON logging setup.
//!
//! Initialises `tracing-subscriber` with:
//! - JSON format (machine-readable)
//! - Timestamps (RFC 3339 via the subscriber's built-in formatter)
//! - Log levels controlled via the `RUST_LOG` environment variable
//! - Request-ID propagation via span fields

use anyhow::Result;
use tracing_subscriber::{fmt, prelude::*, EnvFilter};

/// Initialise the global tracing subscriber with JSON output.
///
/// `default_level` is used when `RUST_LOG` is not set (e.g.
/// `"router_metrics_exporter=info"`).
///
/// Returns an error if a global subscriber has already been installed.
pub fn init_logging(default_level: &str) -> Result<()> {
    let filter =
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(default_level));

    tracing_subscriber::registry()
        .with(
            fmt::layer()
                .json()
                .with_current_span(true)
                .with_span_list(false)
                .with_target(true)
                .with_file(false)
                .with_line_number(false),
        )
        .with(filter)
        .try_init()
        .map_err(|e| anyhow::anyhow!("failed to init tracing: {e}"))?;

    Ok(())
}

/// Generate a new random request ID (UUID v4).
pub fn new_request_id() -> String {
    uuid::Uuid::new_v4().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_id_is_non_empty() {
        let id = new_request_id();
        assert!(!id.is_empty());
    }

    #[test]
    fn request_ids_are_unique() {
        let a = new_request_id();
        let b = new_request_id();
        assert_ne!(a, b);
    }
}

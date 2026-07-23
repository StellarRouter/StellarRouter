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

/// Sanitize a string for safe logging by replacing control characters.
///
/// Replaces newline (`\n`), carriage return (`\r`), and other ASCII control
/// characters (0x00–0x1F, 0x7F) with their Unicode "control pictures" (U+2400–U+241F)
/// to prevent log injection attacks where an attacker embeds newlines or escape
/// sequences in user-controlled fields to forge fake log entries.
///
/// # Example
///
/// ```
/// use router_off_chain_common::logging::sanitize_for_log;
///
/// let malicious = "transfer\nINFO fake_log_entry";
/// let safe = sanitize_for_log(malicious);
/// assert!(!safe.contains('\n'));
/// assert!(safe.contains('␊')); // U+240A SYMBOL FOR LINE FEED
/// ```
pub fn sanitize_for_log(s: &str) -> String {
    s.chars()
        .map(|c| {
            if c.is_ascii_control() {
                // Map ASCII control chars to their Unicode "control pictures"
                // U+2400 (␀) through U+241F (␟) for 0x00–0x1F
                // U+2421 (␡) for DEL (0x7F)
                match c as u8 {
                    0x7F => '\u{2421}', // DEL → ␡
                    b => char::from_u32(0x2400 + b as u32).unwrap_or('�'),
                }
            } else {
                c
            }
        })
        .collect()
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

    #[test]
    fn sanitize_removes_newline() {
        let input = "transfer\nINFO fake_log_entry";
        let output = sanitize_for_log(input);
        assert!(!output.contains('\n'));
        assert!(output.contains('\u{240A}')); // ␊
    }

    #[test]
    fn sanitize_removes_carriage_return() {
        let input = "transfer\rINFO fake_log_entry";
        let output = sanitize_for_log(input);
        assert!(!output.contains('\r'));
        assert!(output.contains('\u{240D}')); // ␍
    }

    #[test]
    fn sanitize_removes_tab() {
        let input = "func\ttab";
        let output = sanitize_for_log(input);
        assert!(!output.contains('\t'));
        assert!(output.contains('\u{2409}')); // ␉
    }

    #[test]
    fn sanitize_preserves_normal_chars() {
        let input = "normal_function-name123";
        let output = sanitize_for_log(input);
        assert_eq!(input, output);
    }

    #[test]
    fn sanitize_handles_del() {
        let input = "func\x7F";
        let output = sanitize_for_log(input);
        assert!(!output.contains('\x7F'));
        assert!(output.contains('\u{2421}')); // ␡
    }
}

//! Background task that polls the Soroban RPC for transaction status and
//! forwards updates into the WebSocket broadcast channel.
//!
//! # How it works
//!
//! [`TxStatusPoller::run`] loops on a configurable interval
//! (`TX_POLL_INTERVAL_MS`, default 2 000 ms). On every tick it reads the
//! current set of watched transaction IDs from `AppState::tx_subscribers`,
//! calls [`SorobanRpcClient::get_transaction_status`] for each, and sends a
//! [`TransactionStatusEvent`] on `AppState::tx_status_tx`.
//!
//! Once a transaction reaches a **terminal** state (`Confirmed` or `Failed`),
//! the poller removes it from `tx_subscribers` so it is never polled again.
//! The WebSocket layer will still receive the final status_update event before
//! the entry is cleaned up.
//!
//! # Configuration
//!
//! | Variable            | Default | Description                              |
//! |---------------------|---------|------------------------------------------|
//! | `TX_POLL_INTERVAL_MS` | `2000` | Milliseconds between polling rounds       |

use std::time::{SystemTime, UNIX_EPOCH};

use tracing::{debug, error, info, warn};

use crate::{
    state::AppState,
    types::{TransactionStatus, TransactionStatusEvent},
};

/// Default polling interval in milliseconds.
const DEFAULT_POLL_INTERVAL_MS: u64 = 2_000;
/// Environment variable that overrides the polling interval.
const POLL_INTERVAL_ENV: &str = "TX_POLL_INTERVAL_MS";

/// Polls the Soroban RPC for the status of every actively-watched transaction
/// and publishes [`TransactionStatusEvent`]s on the broadcast channel.
pub struct TxStatusPoller {
    state: AppState,
    interval_ms: u64,
}

impl TxStatusPoller {
    /// Create a new poller backed by `state`.
    ///
    /// The polling interval is read from `TX_POLL_INTERVAL_MS` at construction
    /// time and falls back to [`DEFAULT_POLL_INTERVAL_MS`] when the variable is
    /// absent or invalid.
    pub fn new(state: AppState) -> Self {
        let interval_ms = parse_poll_interval_ms(std::env::var(POLL_INTERVAL_ENV).ok().as_deref());
        info!(
            interval_ms,
            env = POLL_INTERVAL_ENV,
            "TxStatusPoller initialised"
        );
        Self { state, interval_ms }
    }

    /// Create a poller with an explicit interval (useful for tests).
    pub fn with_interval_ms(state: AppState, interval_ms: u64) -> Self {
        Self { state, interval_ms }
    }

    /// Run the polling loop until the task is cancelled (e.g. the process exits
    /// or the containing `tokio::spawn` is aborted).
    ///
    /// This method never returns under normal operation.
    pub async fn run(self) {
        let interval = tokio::time::Duration::from_millis(self.interval_ms);
        let mut ticker = tokio::time::interval(interval);
        // `MissedTickBehavior::Delay` prevents a burst of back-to-back polls
        // if the system is momentarily busy.
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

        loop {
            ticker.tick().await;
            self.poll_once().await;
        }
    }

    /// Execute a single polling round: iterate `tx_subscribers`, query the RPC
    /// for each tx, emit an event, and remove terminal entries.
    async fn poll_once(&self) {
        // Collect the current tx_ids snapshot to avoid holding the DashMap
        // shard locks across await points.
        let tx_ids: Vec<String> = self
            .state
            .tx_subscribers
            .iter()
            .map(|entry| entry.key().clone())
            .collect();

        if tx_ids.is_empty() {
            debug!("TxStatusPoller: no active subscriptions, skipping poll");
            return;
        }

        info!(count = tx_ids.len(), "TxStatusPoller: polling {} tx(s)", tx_ids.len());

        for tx_id in tx_ids {
            match self.state.rpc.get_transaction_status(&tx_id).await {
                Ok(status) => {
                    let event = TransactionStatusEvent {
                        tx_id: tx_id.clone(),
                        status,
                        timestamp: iso8601_now(),
                        message: status_message(status),
                    };

                    // Emit regardless of whether anyone is listening right now;
                    // the broadcast channel is buffered.
                    if let Err(e) = self.state.tx_status_tx.send(event) {
                        // No active receivers — not an error, just means all WS
                        // connections for this tx disconnected before the event
                        // could be fanned out.
                        debug!(tx_id = %tx_id, "broadcast send skipped (no receivers): {}", e);
                    }

                    // Remove terminal transactions so we stop polling them.
                    if is_terminal(status) {
                        info!(tx_id = %tx_id, ?status, "tx reached terminal state, removing from watch list");
                        self.state.tx_subscribers.remove(&tx_id);
                    }
                }
                Err(e) => {
                    warn!(tx_id = %tx_id, error = %e, "Failed to poll transaction status; will retry next interval");
                }
            }
        }
    }
}

/// Returns `true` for states that will never change (no point polling again).
fn is_terminal(status: TransactionStatus) -> bool {
    matches!(status, TransactionStatus::Confirmed | TransactionStatus::Failed)
}

/// A short human-readable description for each status, forwarded as
/// `message` in the [`TransactionStatusEvent`].
fn status_message(status: TransactionStatus) -> Option<String> {
    let msg = match status {
        TransactionStatus::Pending => "Transaction not yet visible on ledger",
        TransactionStatus::Submitted => "Transaction submitted, awaiting inclusion",
        TransactionStatus::Confirmed => "Transaction confirmed on ledger",
        TransactionStatus::Failed => "Transaction failed",
    };
    Some(msg.to_string())
}

/// Return the current UTC time formatted as an ISO 8601 string
/// (`2026-07-22T00:00:00Z`).
fn iso8601_now() -> String {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    // Format manually to avoid adding a heavy date/time crate dependency.
    // Precision to the second is sufficient for transaction status events.
    seconds_to_iso8601(secs)
}

/// Convert a Unix timestamp (seconds) to a minimal ISO 8601 UTC string.
fn seconds_to_iso8601(secs: u64) -> String {
    // Days since Unix epoch
    let days = secs / 86_400;
    let time_of_day = secs % 86_400;

    let h = time_of_day / 3_600;
    let m = (time_of_day % 3_600) / 60;
    let s = time_of_day % 60;

    // Gregorian calendar calculation (valid for dates after 1970-01-01)
    let (year, month, day) = days_to_ymd(days);

    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z",
        year, month, day, h, m, s
    )
}

/// Convert days since Unix epoch (1970-01-01) to (year, month, day).
fn days_to_ymd(mut days: u64) -> (u64, u64, u64) {
    // Algorithm from https://howardhinnant.github.io/date_algorithms.html
    // "civil_from_days" — public domain.
    let z = days + 719_468;
    let era = z / 146_097;
    let doe = z - era * 146_097; // day of era [0, 146096]
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365; // year of era [0, 399]
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // day of year [0, 365]
    let mp = (5 * doy + 2) / 153; // month prime [0, 11]
    let d = doy - (153 * mp + 2) / 5 + 1; // day [1, 31]
    let m = if mp < 10 { mp + 3 } else { mp - 9 }; // month [1, 12]
    let y = if m <= 2 { y + 1 } else { y };
    (y, m, d)
}

/// Parse `TX_POLL_INTERVAL_MS` from an optional string, falling back to the
/// default when the value is absent, non-numeric, or zero.
fn parse_poll_interval_ms(value: Option<&str>) -> u64 {
    value
        .and_then(|raw| raw.trim().parse::<u64>().ok())
        .filter(|v| *v > 0)
        .unwrap_or(DEFAULT_POLL_INTERVAL_MS)
}

// ─────────────────────────────────────────────────────────────────────────────
// Unit tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_poll_interval_uses_default_for_missing_value() {
        assert_eq!(
            parse_poll_interval_ms(None),
            DEFAULT_POLL_INTERVAL_MS
        );
    }

    #[test]
    fn parse_poll_interval_uses_default_for_zero() {
        assert_eq!(
            parse_poll_interval_ms(Some("0")),
            DEFAULT_POLL_INTERVAL_MS
        );
    }

    #[test]
    fn parse_poll_interval_uses_default_for_invalid_value() {
        assert_eq!(
            parse_poll_interval_ms(Some("not-a-number")),
            DEFAULT_POLL_INTERVAL_MS
        );
    }

    #[test]
    fn parse_poll_interval_accepts_valid_value() {
        assert_eq!(parse_poll_interval_ms(Some("500")), 500);
        assert_eq!(parse_poll_interval_ms(Some("  1000  ")), 1_000);
    }

    #[test]
    fn confirmed_and_failed_are_terminal() {
        assert!(is_terminal(TransactionStatus::Confirmed));
        assert!(is_terminal(TransactionStatus::Failed));
    }

    #[test]
    fn pending_and_submitted_are_not_terminal() {
        assert!(!is_terminal(TransactionStatus::Pending));
        assert!(!is_terminal(TransactionStatus::Submitted));
    }

    #[test]
    fn iso8601_unix_epoch_formats_correctly() {
        assert_eq!(seconds_to_iso8601(0), "1970-01-01T00:00:00Z");
    }

    #[test]
    fn iso8601_known_timestamp_formats_correctly() {
        // 2026-07-22T00:00:00Z = 1753142400 seconds since epoch
        assert_eq!(seconds_to_iso8601(1_753_142_400), "2026-07-22T00:00:00Z");
    }

    #[test]
    fn status_messages_are_present_for_all_variants() {
        for status in [
            TransactionStatus::Pending,
            TransactionStatus::Submitted,
            TransactionStatus::Confirmed,
            TransactionStatus::Failed,
        ] {
            assert!(
                status_message(status).is_some(),
                "missing message for {:?}",
                status
            );
        }
    }
}

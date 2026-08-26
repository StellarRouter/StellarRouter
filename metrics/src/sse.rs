//! Real-time Stellar SSE event subscriber.
//!
//! [`SseSubscriber`] connects to the Stellar Horizon
//! `/accounts/{contract_id}/effects` or the generic
//! `/events?contractId={id}` SSE endpoint, parses the incoming
//! `text/event-stream` lines, and updates Prometheus metrics as events arrive.
//!
//! ## Protocol
//!
//! Stellar Horizon exposes Server-Sent Events as a plain HTTP response with
//! `Content-Type: text/event-stream`.  Each event block is a sequence of
//! `field: value` lines separated by a blank line:
//!
//! ```text
//! data: {"type":"contract","ledger":1234,...}
//!
//! data: {"type":"contract","ledger":1235,...}
//! ```
//!
//! The subscriber reads the byte stream, splits on `\n`, accumulates lines per
//! event block, and dispatches to the metrics handler on each blank-line
//! separator.
//!
//! ## Reconnect strategy
//!
//! The subscriber uses **exponential back-off** on failure:
//!
//! ```text
//! delay = min(base_delay_ms * 2^attempt, max_delay_ms)
//! ```
//!
//! If `max_reconnects > 0` and the reconnect count exceeds it, the subscriber
//! logs an error and terminates (the poll path takes over).
//! If `max_reconnects == 0`, the subscriber retries indefinitely.
//!
//! ## Cancellation
//!
//! Pass a [`tokio_util::sync::CancellationToken`] to stop the subscriber
//! cleanly (e.g. during graceful shutdown or test teardown).

use std::time::Duration;

use anyhow::{Context, Result};
use futures_util::StreamExt;
use reqwest::Client;
use serde::Deserialize;
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, info, warn};

use crate::metrics::RouterMetrics;

// ── SSE event types ──────────────────────────────────────────────────────────

/// A parsed SSE data payload from the Stellar Horizon event stream.
///
/// The schema matches Horizon's `/events` endpoint response.  Unknown fields
/// are silently ignored so the parser is forward-compatible.
#[derive(Debug, Clone, Deserialize)]
pub struct SseEvent {
    /// Ledger sequence number in which this event was emitted.
    #[serde(default)]
    pub ledger: u32,

    /// Contract that emitted this event (strkey format).
    #[serde(rename = "contractId", default)]
    #[allow(dead_code)]
    pub contract_id: String,

    /// Decoded topic symbols (e.g. `["post_call"]`).
    #[serde(default)]
    pub topic: Vec<serde_json::Value>,

    /// Decoded event value payload.
    #[serde(default)]
    pub value: serde_json::Value,

    /// Horizon-assigned event type (usually `"contract"`).
    #[serde(rename = "type", default)]
    pub event_type: String,
}

// ── Subscriber ───────────────────────────────────────────────────────────────

/// Configuration knobs for the SSE subscriber.
#[derive(Debug, Clone)]
pub struct SseConfig {
    /// Horizon base URL, e.g. `https://horizon-testnet.stellar.org`.
    pub horizon_url: String,
    /// Maximum reconnect attempts (0 = unlimited).
    pub max_reconnects: u32,
    /// Base reconnect delay (doubles each attempt).
    pub base_delay: Duration,
    /// Ceiling for exponential back-off.
    pub max_delay: Duration,
    /// HTTP request timeout for each SSE connection attempt.
    pub connect_timeout: Duration,
}

impl Default for SseConfig {
    fn default() -> Self {
        Self {
            horizon_url: "https://horizon-testnet.stellar.org".to_string(),
            max_reconnects: 10,
            base_delay: Duration::from_secs(1),
            max_delay: Duration::from_secs(30),
            connect_timeout: Duration::from_secs(10),
        }
    }
}

/// Subscribes to the Stellar Horizon SSE event stream for a single contract and
/// updates Prometheus metrics as events arrive.
///
/// Designed to run in a long-lived `tokio::spawn` task.  Graceful shutdown is
/// triggered by cancelling the provided [`CancellationToken`].
pub struct SseSubscriber {
    config: SseConfig,
    contract_id: String,
    metrics: RouterMetrics,
    http: Client,
    cancel: CancellationToken,
}

impl SseSubscriber {
    /// Create a new subscriber.
    ///
    /// The subscriber does not connect until [`Self::run`] is called.
    pub fn new(
        config: SseConfig,
        contract_id: impl Into<String>,
        metrics: RouterMetrics,
        cancel: CancellationToken,
    ) -> Result<Self> {
        let http = Client::builder()
            .timeout(config.connect_timeout)
            // SSE streams are long-lived; disable the per-request timeout so we
            // don't time out the *stream* itself — only the initial connect.
            .connection_verbose(false)
            .build()
            .context("failed to build SSE HTTP client")?;

        Ok(Self {
            config,
            contract_id: contract_id.into(),
            metrics,
            http,
            cancel,
        })
    }

    /// Build the Horizon SSE URL for this contract's events.
    ///
    /// Uses the `/events` endpoint with a contract ID filter and `cursor=now`
    /// so we only receive events that occur after the subscription starts.
    pub fn sse_url(&self) -> String {
        format!(
            "{}/events?contractId={}&cursor=now",
            self.config.horizon_url.trim_end_matches('/'),
            self.contract_id
        )
    }

    /// Run the subscriber loop until cancelled or max reconnects exceeded.
    ///
    /// On each successful connect the function reads the SSE byte stream,
    /// dispatches events to [`Self::handle_event`], and updates the
    /// `sse_connected` / `sse_events_total` metrics.
    ///
    /// On error it sleeps for the back-off duration, increments
    /// `sse_reconnects_total`, and retries.
    pub async fn run(self) {
        let contract_id = self.contract_id.clone();
        let max_reconnects = self.config.max_reconnects;
        let base_delay = self.config.base_delay;
        let max_delay = self.config.max_delay;

        let mut reconnect_count: u32 = 0;

        info!(contract_id, "SSE subscriber starting");

        loop {
            // Check cancellation before each (re)connect attempt.
            if self.cancel.is_cancelled() {
                info!(contract_id, "SSE subscriber cancelled");
                self.metrics
                    .sse_connected
                    .with_label_values(&[&contract_id])
                    .set(0.0);
                break;
            }

            // Enforce max reconnects (0 = unlimited).
            if max_reconnects > 0 && reconnect_count >= max_reconnects {
                error!(
                    contract_id,
                    reconnect_count, "SSE subscriber exceeded max reconnects — giving up"
                );
                self.metrics
                    .sse_connected
                    .with_label_values(&[&contract_id])
                    .set(0.0);
                break;
            }

            let url = self.sse_url();
            info!(contract_id, %url, "SSE subscriber connecting");

            match self.connect_and_stream(&url).await {
                Ok(()) => {
                    // Stream ended cleanly (e.g. server closed connection).
                    warn!(contract_id, "SSE stream ended — reconnecting");
                }
                Err(e) => {
                    warn!(contract_id, error = %e, "SSE stream error — reconnecting");
                }
            }

            self.metrics
                .sse_connected
                .with_label_values(&[&contract_id])
                .set(0.0);

            reconnect_count += 1;
            self.metrics
                .sse_reconnects_total
                .with_label_values(&[&contract_id])
                .inc();

            // Exponential back-off, capped at max_delay.
            let delay = std::cmp::min(
                base_delay
                    .checked_mul(2u32.saturating_pow(reconnect_count.saturating_sub(1)))
                    .unwrap_or(max_delay),
                max_delay,
            );

            info!(
                contract_id,
                reconnect_count,
                delay_ms = delay.as_millis(),
                "SSE back-off before reconnect"
            );

            // Sleep or cancel.
            tokio::select! {
                _ = tokio::time::sleep(delay) => {}
                _ = self.cancel.cancelled() => {
                    info!(contract_id, "SSE subscriber cancelled during back-off");
                    break;
                }
            }
        }

        info!(contract_id, "SSE subscriber terminated");
    }

    /// Open the SSE connection, read the stream, and dispatch events.
    ///
    /// Returns `Ok(())` when the stream ends cleanly, or `Err(_)` on failure.
    async fn connect_and_stream(&self, url: &str) -> Result<()> {
        let contract_id = &self.contract_id;

        let response = self
            .http
            .get(url)
            .header("Accept", "text/event-stream")
            .send()
            .await
            .context("SSE HTTP request failed")?;

        let status = response.status();
        if !status.is_success() {
            return Err(anyhow::anyhow!(
                "SSE endpoint returned HTTP {status} for {url}"
            ));
        }

        self.metrics
            .sse_connected
            .with_label_values(&[contract_id])
            .set(1.0);
        info!(contract_id, "SSE stream connected");

        // Read the response body as a byte stream.
        let mut byte_stream = response.bytes_stream();
        let mut line_buf = String::new();
        let mut event_lines: Vec<String> = Vec::new();

        loop {
            tokio::select! {
                chunk = byte_stream.next() => {
                    match chunk {
                        None => {
                            // End of stream.
                            return Ok(());
                        }
                        Some(Err(e)) => {
                            return Err(anyhow::anyhow!("SSE stream read error: {e}"));
                        }
                        Some(Ok(bytes)) => {
                            // Append to the line buffer and process complete lines.
                            let text = String::from_utf8_lossy(&bytes);
                            line_buf.push_str(&text);

                            while let Some(newline_pos) = line_buf.find('\n') {
                                let line = line_buf[..newline_pos]
                                    .trim_end_matches('\r')
                                    .to_string();
                                line_buf.drain(..=newline_pos);

                                if line.is_empty() {
                                    // Blank line = end of event block; dispatch.
                                    if !event_lines.is_empty() {
                                        self.dispatch_event_block(&event_lines);
                                        event_lines.clear();
                                    }
                                } else {
                                    event_lines.push(line);
                                }
                            }
                        }
                    }
                }
                _ = self.cancel.cancelled() => {
                    info!(contract_id, "SSE stream cancelled mid-stream");
                    return Ok(());
                }
            }
        }
    }

    /// Parse and dispatch a single SSE event block (a group of `field: value` lines).
    fn dispatch_event_block(&self, lines: &[String]) {
        let contract_id = &self.contract_id;

        // Extract the `data:` field from the event block.
        let data = lines
            .iter()
            .find(|l| l.starts_with("data:"))
            .map(|l| l["data:".len()..].trim().to_string());

        let data = match data {
            Some(d) if !d.is_empty() => d,
            _ => {
                debug!(contract_id, "SSE event block has no data field — skipping");
                return;
            }
        };

        // Parse the JSON payload.
        match serde_json::from_str::<SseEvent>(&data) {
            Ok(event) => {
                debug!(
                    contract_id,
                    ledger = event.ledger,
                    event_type = %event.event_type,
                    "SSE event received"
                );
                self.metrics
                    .sse_events_total
                    .with_label_values(&[contract_id])
                    .inc();
                self.handle_event(event);
            }
            Err(e) => {
                debug!(contract_id, error = %e, raw = %data, "SSE event JSON parse failed");
            }
        }
    }

    /// Update Prometheus metrics based on the event type and payload.
    ///
    /// Topic-based dispatch mirrors the polling path in `collector.rs`.
    pub fn handle_event(&self, event: SseEvent) {
        let contract_id = &self.contract_id;

        // Extract the first topic symbol to identify the event kind.
        let topic = event
            .topic
            .first()
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string();

        match topic.as_str() {
            // ── router-middleware: post_call ──────────────────────────────────
            "post_call" => {
                let success = event.value.get("success").and_then(|v| v.as_bool());
                let route = event
                    .value
                    .get("route")
                    .and_then(|v| v.as_str())
                    .unwrap_or("unknown")
                    .to_string();

                self.metrics
                    .middleware_route_calls_total
                    .with_label_values(&[contract_id.as_str(), route.as_str()])
                    .inc();

                if success == Some(false) {
                    self.metrics
                        .middleware_route_failures_total
                        .with_label_values(&[contract_id.as_str(), route.as_str()])
                        .inc();
                }
            }

            // ── router-quote: quote_generated ─────────────────────────────────
            "quote_generated" => {
                self.metrics
                    .quote_total_generated
                    .with_label_values(&[contract_id.as_str()])
                    .inc();
            }

            // ── router-quote: fee_estimated ───────────────────────────────────
            "fee_estimated" => {
                self.metrics
                    .quote_total_fee_estimated
                    .with_label_values(&[contract_id.as_str()])
                    .inc();
            }

            // ── router-execution: execution_result ────────────────────────────
            "execution_result" => {
                self.metrics
                    .execution_total_executions
                    .with_label_values(&[contract_id.as_str()])
                    .inc();
            }

            // ── router-execution: execution_error ─────────────────────────────
            "execution_error" => {
                self.metrics
                    .execution_total_errors
                    .with_label_values(&[contract_id.as_str()])
                    .inc();
            }

            other => {
                debug!(
                    contract_id,
                    topic = other,
                    "SSE event with unrecognised topic — ignoring"
                );
            }
        }
    }
}

// ── Helpers ───────────────────────────────────────────────────────────────────

/// Build an [`SseConfig`] from [`crate::cli::Args`].
pub fn sse_config_from_args(args: &crate::cli::Args) -> SseConfig {
    SseConfig {
        horizon_url: args.horizon_url.clone(),
        max_reconnects: args.sse_max_reconnects,
        base_delay: Duration::from_millis(args.sse_reconnect_delay_ms),
        max_delay: Duration::from_millis(args.sse_reconnect_max_delay_ms),
        connect_timeout: Duration::from_secs(args.rpc_timeout_secs),
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use prometheus::Registry;
    use serde_json::json;
    use std::sync::Arc;
    use tokio_util::sync::CancellationToken;

    fn make_subscriber(contract_id: &str) -> (SseSubscriber, RouterMetrics) {
        make_subscriber_with_config(contract_id, SseConfig::default())
    }

    fn make_subscriber_with_config(
        contract_id: &str,
        config: SseConfig,
    ) -> (SseSubscriber, RouterMetrics) {
        let reg = Registry::new();
        let metrics = RouterMetrics::new(&reg).unwrap();
        let cancel = CancellationToken::new();
        let sub = SseSubscriber::new(config, contract_id, metrics.clone(), cancel).unwrap();
        (sub, metrics)
    }

    // ── Event parsing tests ───────────────────────────────────────────────────

    #[test]
    fn test_handle_event_quote_generated_increments_counter() {
        let (sub, metrics) = make_subscriber("C1");

        let event = SseEvent {
            ledger: 100,
            contract_id: "C1".to_string(),
            topic: vec![json!("quote_generated")],
            value: json!({}),
            event_type: "contract".to_string(),
        };

        sub.handle_event(event);

        assert_eq!(
            metrics
                .quote_total_generated
                .with_label_values(&["C1"])
                .get(),
            1.0
        );
        assert_eq!(
            metrics
                .quote_total_fee_estimated
                .with_label_values(&["C1"])
                .get(),
            0.0
        );
    }

    #[test]
    fn test_handle_event_fee_estimated_increments_counter() {
        let (sub, metrics) = make_subscriber("C1");

        let event = SseEvent {
            ledger: 101,
            contract_id: "C1".to_string(),
            topic: vec![json!("fee_estimated")],
            value: json!({}),
            event_type: "contract".to_string(),
        };

        sub.handle_event(event);

        assert_eq!(
            metrics
                .quote_total_fee_estimated
                .with_label_values(&["C1"])
                .get(),
            1.0
        );
    }

    #[test]
    fn test_handle_event_execution_result_increments_counter() {
        let (sub, metrics) = make_subscriber("EXEC");

        sub.handle_event(SseEvent {
            ledger: 200,
            contract_id: "EXEC".to_string(),
            topic: vec![json!("execution_result")],
            value: json!({}),
            event_type: "contract".to_string(),
        });

        assert_eq!(
            metrics
                .execution_total_executions
                .with_label_values(&["EXEC"])
                .get(),
            1.0
        );
    }

    #[test]
    fn test_handle_event_execution_error_increments_counter() {
        let (sub, metrics) = make_subscriber("EXEC");

        sub.handle_event(SseEvent {
            ledger: 201,
            contract_id: "EXEC".to_string(),
            topic: vec![json!("execution_error")],
            value: json!({}),
            event_type: "contract".to_string(),
        });

        assert_eq!(
            metrics
                .execution_total_errors
                .with_label_values(&["EXEC"])
                .get(),
            1.0
        );
    }

    #[test]
    fn test_handle_event_post_call_success_increments_calls_not_failures() {
        let (sub, metrics) = make_subscriber("MW");

        sub.handle_event(SseEvent {
            ledger: 300,
            contract_id: "MW".to_string(),
            topic: vec![json!("post_call")],
            value: json!({ "route": "oracle", "success": true }),
            event_type: "contract".to_string(),
        });

        assert_eq!(
            metrics
                .middleware_route_calls_total
                .with_label_values(&["MW", "oracle"])
                .get(),
            1.0
        );
        assert_eq!(
            metrics
                .middleware_route_failures_total
                .with_label_values(&["MW", "oracle"])
                .get(),
            0.0
        );
    }

    #[test]
    fn test_handle_event_post_call_failure_increments_failures() {
        let (sub, metrics) = make_subscriber("MW");

        sub.handle_event(SseEvent {
            ledger: 301,
            contract_id: "MW".to_string(),
            topic: vec![json!("post_call")],
            value: json!({ "route": "vault", "success": false }),
            event_type: "contract".to_string(),
        });

        assert_eq!(
            metrics
                .middleware_route_calls_total
                .with_label_values(&["MW", "vault"])
                .get(),
            1.0
        );
        assert_eq!(
            metrics
                .middleware_route_failures_total
                .with_label_values(&["MW", "vault"])
                .get(),
            1.0
        );
    }

    #[test]
    fn test_handle_event_unknown_topic_is_ignored() {
        let (sub, metrics) = make_subscriber("C1");

        sub.handle_event(SseEvent {
            ledger: 400,
            contract_id: "C1".to_string(),
            topic: vec![json!("some_unknown_event")],
            value: json!({ "data": "irrelevant" }),
            event_type: "contract".to_string(),
        });

        // No counters should have been incremented.
        assert_eq!(
            metrics.sse_events_total.with_label_values(&["C1"]).get(),
            0.0
        );
    }

    #[test]
    fn test_handle_event_empty_topic_is_ignored() {
        let (sub, _metrics) = make_subscriber("C1");

        // Should not panic.
        sub.handle_event(SseEvent {
            ledger: 0,
            contract_id: "C1".to_string(),
            topic: vec![],
            value: json!({}),
            event_type: "contract".to_string(),
        });
    }

    // ── dispatch_event_block tests ────────────────────────────────────────────

    #[test]
    fn test_dispatch_event_block_parses_data_line() {
        let (sub, metrics) = make_subscriber("C1");

        let lines = vec![
            "event: contract".to_string(),
            r#"data: {"ledger":100,"contractId":"C1","topic":["quote_generated"],"value":{},"type":"contract"}"#
                .to_string(),
        ];

        sub.dispatch_event_block(&lines);

        assert_eq!(
            metrics.sse_events_total.with_label_values(&["C1"]).get(),
            1.0
        );
        assert_eq!(
            metrics
                .quote_total_generated
                .with_label_values(&["C1"])
                .get(),
            1.0
        );
    }

    #[test]
    fn test_dispatch_event_block_skips_empty_data() {
        let (sub, metrics) = make_subscriber("C1");

        // Only a comment line, no data.
        let lines = vec![": keep-alive".to_string()];
        sub.dispatch_event_block(&lines);

        assert_eq!(
            metrics.sse_events_total.with_label_values(&["C1"]).get(),
            0.0
        );
    }

    #[test]
    fn test_dispatch_event_block_handles_invalid_json() {
        let (sub, metrics) = make_subscriber("C1");

        // Invalid JSON in data field — should not panic or increment sse_events_total.
        let lines = vec!["data: {not valid json}".to_string()];
        sub.dispatch_event_block(&lines);

        assert_eq!(
            metrics.sse_events_total.with_label_values(&["C1"]).get(),
            0.0
        );
    }

    // ── Reconnect / back-off tests ────────────────────────────────────────────

    #[test]
    fn test_sse_url_format() {
        let config = SseConfig {
            horizon_url: "https://horizon-testnet.stellar.org".to_string(),
            ..Default::default()
        };
        let (sub, _) = make_subscriber_with_config("CONTRACT123", config);
        assert_eq!(
            sub.sse_url(),
            "https://horizon-testnet.stellar.org/events?contractId=CONTRACT123&cursor=now"
        );
    }

    #[test]
    fn test_sse_url_strips_trailing_slash() {
        let config = SseConfig {
            horizon_url: "https://horizon-testnet.stellar.org/".to_string(),
            ..Default::default()
        };
        let (sub, _) = make_subscriber_with_config("C1", config);
        assert_eq!(
            sub.sse_url(),
            "https://horizon-testnet.stellar.org/events?contractId=C1&cursor=now"
        );
    }

    /// Verify that the subscriber stops after `max_reconnects` attempts when
    /// the endpoint is unreachable.
    #[tokio::test]
    async fn test_subscriber_stops_after_max_reconnects() {
        // Use an unreachable URL so every connect attempt fails immediately.
        let config = SseConfig {
            horizon_url: "http://127.0.0.1:1".to_string(), // port 1 — always refused
            max_reconnects: 2,
            base_delay: Duration::from_millis(1),
            max_delay: Duration::from_millis(2),
            connect_timeout: Duration::from_millis(50),
        };

        let reg = Registry::new();
        let metrics = RouterMetrics::new(&reg).unwrap();
        let cancel = CancellationToken::new();

        let sub = SseSubscriber::new(config, "C1", metrics.clone(), cancel.clone()).unwrap();

        // run() should terminate on its own after max_reconnects exceeded.
        tokio::time::timeout(Duration::from_secs(5), sub.run())
            .await
            .expect("subscriber should have terminated within 5 seconds");

        // Reconnect counter should be >= max_reconnects.
        let reconnects = metrics
            .sse_reconnects_total
            .with_label_values(&["C1"])
            .get();
        assert!(
            reconnects >= 2.0,
            "expected ≥ 2 reconnects, got {reconnects}"
        );

        // Connected gauge should be 0 after termination.
        assert_eq!(metrics.sse_connected.with_label_values(&["C1"]).get(), 0.0);
    }

    /// Verify that the subscriber stops immediately when cancelled before any
    /// connect attempt.
    #[tokio::test]
    async fn test_subscriber_respects_cancellation_before_connect() {
        let config = SseConfig {
            horizon_url: "http://127.0.0.1:1".to_string(),
            max_reconnects: 100,
            base_delay: Duration::from_millis(1),
            max_delay: Duration::from_millis(1),
            connect_timeout: Duration::from_millis(50),
        };

        let reg = Registry::new();
        let metrics = RouterMetrics::new(&reg).unwrap();
        let cancel = CancellationToken::new();

        // Cancel before run() is called.
        cancel.cancel();

        let sub = SseSubscriber::new(config, "C1", metrics.clone(), cancel).unwrap();

        tokio::time::timeout(Duration::from_secs(1), sub.run())
            .await
            .expect("cancelled subscriber should exit immediately");

        // No reconnects should have happened.
        assert_eq!(
            metrics
                .sse_reconnects_total
                .with_label_values(&["C1"])
                .get(),
            0.0
        );
    }

    /// Verify that the subscriber stops cleanly when cancelled during the
    /// back-off sleep.
    #[tokio::test]
    async fn test_subscriber_cancels_during_backoff() {
        let config = SseConfig {
            horizon_url: "http://127.0.0.1:1".to_string(),
            max_reconnects: 100,
            // Long base delay so we can cancel mid-backoff.
            base_delay: Duration::from_secs(60),
            max_delay: Duration::from_secs(60),
            connect_timeout: Duration::from_millis(50),
        };

        let reg = Registry::new();
        let metrics = RouterMetrics::new(&reg).unwrap();
        let cancel = CancellationToken::new();

        let sub = SseSubscriber::new(config, "C1", metrics.clone(), cancel.clone()).unwrap();

        // Cancel after a short delay so the subscriber is mid-backoff.
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(200)).await;
            cancel.cancel();
        });

        tokio::time::timeout(Duration::from_secs(3), sub.run())
            .await
            .expect("subscriber should exit shortly after cancel");
    }

    /// Full integration test: spawn a minimal SSE HTTP server and verify the
    /// subscriber parses and dispatches the events correctly.
    #[tokio::test]
    async fn test_subscriber_parses_sse_stream_from_mock_server() {
        use axum::response::sse::{Event, Sse};
        use futures_util::stream;

        // Build a minimal SSE payload with one quote_generated event.
        let sse_payload = concat!(
            "event: contract\n",
            r#"data: {"ledger":1,"contractId":"C1","topic":["quote_generated"],"value":{},"type":"contract"}"#,
            "\n\n",
            "event: contract\n",
            r#"data: {"ledger":2,"contractId":"C1","topic":["fee_estimated"],"value":{},"type":"contract"}"#,
            "\n\n"
        );

        let sse_bytes = sse_payload.as_bytes().to_vec();

        let app = axum::Router::new().route(
            "/events",
            axum::routing::get(move || {
                let body = sse_bytes.clone();
                async move {
                    axum::response::Response::builder()
                        .header("Content-Type", "text/event-stream")
                        .header("Cache-Control", "no-cache")
                        .body(axum::body::Body::from(body))
                        .unwrap()
                }
            }),
        );

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        let config = SseConfig {
            horizon_url: format!("http://{addr}"),
            max_reconnects: 1,
            base_delay: Duration::from_millis(1),
            max_delay: Duration::from_millis(1),
            connect_timeout: Duration::from_secs(2),
        };

        let reg = Registry::new();
        let metrics = RouterMetrics::new(&reg).unwrap();
        let cancel = CancellationToken::new();

        let sub = SseSubscriber::new(config, "C1", metrics.clone(), cancel).unwrap();

        tokio::time::timeout(Duration::from_secs(5), sub.run())
            .await
            .expect("subscriber should finish within 5 seconds");

        // Both events should have been counted.
        assert_eq!(
            metrics
                .quote_total_generated
                .with_label_values(&["C1"])
                .get(),
            1.0,
            "expected 1 quote_generated event"
        );
        assert_eq!(
            metrics
                .quote_total_fee_estimated
                .with_label_values(&["C1"])
                .get(),
            1.0,
            "expected 1 fee_estimated event"
        );
        assert_eq!(
            metrics.sse_events_total.with_label_values(&["C1"]).get(),
            2.0,
            "expected 2 total SSE events"
        );
    }

    // ── sse_config_from_args ──────────────────────────────────────────────────

    #[test]
    fn test_sse_config_from_args() {
        use crate::cli::{Args, EventMode};
        use clap::Parser;

        let args = Args::parse_from([
            "router-metrics-exporter",
            "--horizon-url",
            "https://horizon-testnet.stellar.org",
            "--sse-max-reconnects",
            "5",
            "--sse-reconnect-delay-ms",
            "500",
            "--sse-reconnect-max-delay-ms",
            "15000",
            "--rpc-timeout-secs",
            "8",
        ]);

        let config = sse_config_from_args(&args);
        assert_eq!(config.horizon_url, "https://horizon-testnet.stellar.org");
        assert_eq!(config.max_reconnects, 5);
        assert_eq!(config.base_delay, Duration::from_millis(500));
        assert_eq!(config.max_delay, Duration::from_millis(15000));
        assert_eq!(config.connect_timeout, Duration::from_secs(8));
    }
}

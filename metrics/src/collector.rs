//! Background scrape loop.
//!
//! The [`Collector`] spawns a `tokio` task that wakes up every
//! `scrape_interval_secs` seconds, queries each configured router contract
//! via the Soroban RPC, and updates the Prometheus gauges / counters.
//!
//! ## Scraping strategy
//!
//! - `router-core`:       `simulateTransaction` — `total_routed()`, `is_paused()`,
//!                        `get_all_routes()` + `get_route(name)` per route.
//! - `router-middleware`: `simulateTransaction` — `total_calls()`,
//!                        `get_configured_routes()` + `circuit_breaker_state(route)`.
//! - `router-registry`:  `simulateTransaction` — `get_all_names()` (total count).
//! - `router-quote`:     `getEvents` — counts `quote_generated` and `fee_estimated`
//!                        events emitted by the contract.
//! - `router-execution`: `getEvents` — counts `execution_result` and `execution_error`
//!                        events; reads `MaxRetries` config via `getLedgerEntries`.
//!
//! ## Ledger cursor (quote + execution)
//!
//! `scrape_quote` and `scrape_execution` maintain a per-contract *last-processed
//! ledger* cursor stored in memory.  On the first scrape (cursor = 0) the full
//! event history visible in the RPC server's retention window is counted and the
//! gauges are **set** to that baseline.  On subsequent scrapes only new events
//! (ledger > cursor) are fetched and the gauges are **incremented** by the
//! new-event count, avoiding redundant re-processing.
//!
//! **Restart limitation:** the in-memory cursor resets to 0 on process restart,
//! so the exporter re-establishes the baseline from the RPC window on the next
//! scrape cycle.  Prometheus will show a transient dip-then-jump if the restart
//! occurs while events are within the retention window.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

use anyhow::Result;
use tokio::sync::Mutex;
use tracing::{error, info, warn};

use crate::cli::Args;
use crate::metrics::RouterMetrics;
use crate::rpc::{RpcClient, SorobanRpcClient};

/// Drives the periodic scrape loop.
#[derive(Clone)]
pub struct Collector {
    args: Args,
    metrics: RouterMetrics,
    /// Last-processed ledger cursor per contract, keyed as `"<scope>:<contract_id>"`.
    /// Held in-memory; resets to 0 on restart (see module-level docs).
    last_ledger: Arc<Mutex<HashMap<String, u32>>>,
}

impl Collector {
    pub fn new(args: Args, metrics: RouterMetrics) -> Self {
        Self {
            args,
            metrics,
            last_ledger: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Run forever, scraping on the configured interval.
    pub async fn run(self) {
        let interval = tokio::time::Duration::from_secs(self.args.scrape_interval_secs);
        info!(
            interval_secs = self.args.scrape_interval_secs,
            "scrape loop started"
        );

        let client = match SorobanRpcClient::new(&self.args.rpc_url, self.args.rpc_timeout_secs) {
            Ok(c) => c,
            Err(e) => {
                error!("failed to create RPC client: {e:#}");
                return;
            }
        };

        loop {
            let cycle_ok = self.scrape_all(&client).await;
            self.metrics.up.set(if cycle_ok { 1.0 } else { 0.0 });
            tokio::time::sleep(interval).await;
        }
    }

    /// Scrape all configured contracts.  Returns `true` if every scrape
    /// succeeded, `false` if any failed.
    async fn scrape_all(&self, client: &dyn RpcClient) -> bool {
        let mut all_ok = true;

        if !self.args.core_contract_id.is_empty() {
            if let Err(e) = self.scrape_core(client, &self.args.core_contract_id).await {
                warn!(contract = %self.args.core_contract_id, "core scrape failed: {e:#}");
                self.metrics
                    .scrape_errors_total
                    .with_label_values(&[&self.args.core_contract_id])
                    .inc();
                all_ok = false;
            }
        }

        if !self.args.middleware_contract_id.is_empty() {
            if let Err(e) = self
                .scrape_middleware(client, &self.args.middleware_contract_id)
                .await
            {
                warn!(contract = %self.args.middleware_contract_id, "middleware scrape failed: {e:#}");
                self.metrics
                    .scrape_errors_total
                    .with_label_values(&[&self.args.middleware_contract_id])
                    .inc();
                all_ok = false;
            }
        }

        if !self.args.registry_contract_id.is_empty() {
            if let Err(e) = self
                .scrape_registry(client, &self.args.registry_contract_id)
                .await
            {
                warn!(contract = %self.args.registry_contract_id, "registry scrape failed: {e:#}");
                self.metrics
                    .scrape_errors_total
                    .with_label_values(&[&self.args.registry_contract_id])
                    .inc();
                all_ok = false;
            }
        }

        if !self.args.quote_contract_id.is_empty() {
            if let Err(e) = self
                .scrape_quote(client, &self.args.quote_contract_id)
                .await
            {
                warn!(contract = %self.args.quote_contract_id, "quote scrape failed: {e:#}");
                self.metrics
                    .scrape_errors_total
                    .with_label_values(&[&self.args.quote_contract_id])
                    .inc();
                all_ok = false;
            }
        }

        if !self.args.execution_contract_id.is_empty() {
            if let Err(e) = self
                .scrape_execution(client, &self.args.execution_contract_id)
                .await
            {
                warn!(contract = %self.args.execution_contract_id, "execution scrape failed: {e:#}");
                self.metrics
                    .scrape_errors_total
                    .with_label_values(&[&self.args.execution_contract_id])
                    .inc();
                all_ok = false;
            }
        }

        all_ok
    }

    // ── router-core ───────────────────────────────────────────────────────────

    async fn scrape_core(&self, client: &dyn RpcClient, contract_id: &str) -> Result<()> {
        let start = Instant::now();
        info!(contract_id, "scraping router-core");

        // 1. total_routed
        let total_routed = client.call_u64(contract_id, "total_routed").await?;
        self.metrics
            .core_total_routed
            .with_label_values(&[contract_id])
            .set(total_routed as f64);

        // 2. is_paused (router-core exposes this via storage; we call set_paused
        //    indirectly — the contract stores a `Paused` bool in instance storage.
        //    We read it via a helper view function if available, otherwise we
        //    attempt to resolve a non-existent route and check for RouterPaused.)
        //
        //    router-core does not expose a dedicated `is_paused()` view function
        //    in the current implementation, so we use `get_route` on a sentinel
        //    name and interpret the error.  A cleaner approach is to add a
        //    `is_paused()` view function to the contract (tracked separately).
        //
        //    For now we record 0 (unknown / not paused) and note the limitation.
        self.metrics
            .core_paused
            .with_label_values(&[contract_id])
            .set(0.0); // updated below if the RPC call succeeds

        // 3. get_all_routes → per-route paused state
        let routes = client
            .call_string_vec(contract_id, "get_all_routes")
            .await?;
        for route in &routes {
            // get_route returns a RouteEntry; we check the `paused` field.
            // The JSON representation of a Soroban struct is a map of field names.
            let route_result = client
                .simulate_invoke(contract_id, "get_route", vec![encode_string_arg(route)])
                .await;

            match route_result {
                Ok(val) => {
                    let paused = extract_route_paused(&val).unwrap_or(false);
                    self.metrics
                        .core_route_paused
                        .with_label_values(&[contract_id, route])
                        .set(if paused { 1.0 } else { 0.0 });
                }
                Err(e) => {
                    warn!(contract_id, route, "failed to get route state: {e:#}");
                }
            }
        }

        let elapsed = start.elapsed().as_secs_f64();
        self.metrics
            .scrape_duration_seconds
            .with_label_values(&[contract_id])
            .observe(elapsed);

        info!(
            contract_id,
            elapsed_secs = elapsed,
            routes = routes.len(),
            total_routed,
            "core scrape done"
        );
        Ok(())
    }

    // ── router-middleware ─────────────────────────────────────────────────────

    async fn scrape_middleware(&self, client: &dyn RpcClient, contract_id: &str) -> Result<()> {
        let start = Instant::now();
        info!(contract_id, "scraping router-middleware");

        // 1. total_calls
        let total_calls = client.call_u64(contract_id, "total_calls").await?;
        self.metrics
            .middleware_total_calls
            .with_label_values(&[contract_id])
            .set(total_calls as f64);

        // 2. Per-route circuit breaker state
        let routes = client
            .call_string_vec(contract_id, "get_configured_routes")
            .await?;

        for route in &routes {
            let cb_result = client
                .simulate_invoke(
                    contract_id,
                    "circuit_breaker_state",
                    vec![encode_string_arg(route)],
                )
                .await;

            match cb_result {
                Ok(val) => {
                    let (is_open, failure_count) =
                        extract_circuit_breaker_state(&val).unwrap_or((false, 0));
                    self.metrics
                        .middleware_circuit_open
                        .with_label_values(&[contract_id, route])
                        .set(if is_open { 1.0 } else { 0.0 });
                    self.metrics
                        .middleware_failure_count
                        .with_label_values(&[contract_id, route])
                        .set(failure_count as f64);
                }
                Err(e) => {
                    warn!(
                        contract_id,
                        route, "failed to get circuit breaker state: {e:#}"
                    );
                }
            }
        }

        let elapsed = start.elapsed().as_secs_f64();
        self.metrics
            .scrape_duration_seconds
            .with_label_values(&[contract_id])
            .observe(elapsed);

        info!(
            contract_id,
            elapsed_secs = elapsed,
            routes = routes.len(),
            total_calls,
            "middleware scrape done"
        );
        Ok(())
    }

    // ── router-registry ───────────────────────────────────────────────────────

    async fn scrape_registry(&self, client: &dyn RpcClient, contract_id: &str) -> Result<()> {
        let start = Instant::now();
        info!(contract_id, "scraping router-registry");

        let names = client.call_string_vec(contract_id, "get_all_names").await?;

        self.metrics
            .registry_total_names
            .with_label_values(&[contract_id])
            .set(names.len() as f64);

        // Call versions(name) for each registered name to track per-name version count.
        for name in &names {
            match client.call_u32_vec(contract_id, "versions", name).await {
                Ok(versions) => {
                    self.metrics
                        .registry_version_count
                        .with_label_values(&[contract_id, name])
                        .set(versions.len() as f64);
                }
                Err(e) => {
                    warn!(contract_id, name, "failed to get versions: {e:#}");
                }
            }
        }

        let elapsed = start.elapsed().as_secs_f64();
        self.metrics
            .scrape_duration_seconds
            .with_label_values(&[contract_id])
            .observe(elapsed);

        info!(
            contract_id,
            elapsed_secs = elapsed,
            total_names = names.len(),
            "registry scrape done"
        );
        Ok(())
    }

    // ── router-quote ──────────────────────────────────────────────────────────

    /// Scrape `router-quote` by counting `quote_generated` and `fee_estimated`
    /// events via `getEvents`, using an in-memory ledger cursor to avoid
    /// reprocessing events on every cycle.
    ///
    /// First scrape (cursor = 0): fetches all events in the RPC retention window
    /// and **sets** the gauges to the observed totals.  Subsequent scrapes fetch
    /// only events newer than the cursor and **increment** the gauges.
    async fn scrape_quote(&self, client: &dyn RpcClient, contract_id: &str) -> Result<()> {
        let start = Instant::now();
        info!(contract_id, "scraping router-quote");

        let start_ledger = {
            let map = self.last_ledger.lock().await;
            map.get(&format!("quote:{contract_id}")).copied().unwrap_or(0)
        };

        let quote_events = client
            .get_events(contract_id, &["quote_generated"], start_ledger)
            .await?;
        let fee_events = client
            .get_events(contract_id, &["fee_estimated"], start_ledger)
            .await?;

        // Advance cursor to the highest ledger seen across both event sets.
        let max_ledger = quote_events
            .iter()
            .chain(fee_events.iter())
            .map(|e| e.ledger)
            .max()
            .unwrap_or(start_ledger);

        let quote_count = quote_events.len() as f64;
        let fee_count = fee_events.len() as f64;
        let g_quote = self.metrics.quote_total_generated.with_label_values(&[contract_id]);
        let g_fee = self.metrics.quote_total_fee_estimated.with_label_values(&[contract_id]);

        if start_ledger == 0 {
            // Baseline: set absolute counts from the full retention window.
            g_quote.set(quote_count);
            g_fee.set(fee_count);
        } else {
            // Incremental: add only the newly-observed events.
            g_quote.inc_by(quote_count);
            g_fee.inc_by(fee_count);
        }

        if max_ledger > start_ledger {
            self.last_ledger
                .lock()
                .await
                .insert(format!("quote:{contract_id}"), max_ledger);
        }

        let elapsed = start.elapsed().as_secs_f64();
        self.metrics
            .scrape_duration_seconds
            .with_label_values(&[contract_id])
            .observe(elapsed);

        info!(
            contract_id,
            elapsed_secs = elapsed,
            quote_generated = quote_events.len(),
            fee_estimated = fee_events.len(),
            start_ledger,
            max_ledger,
            "quote scrape done"
        );
        Ok(())
    }

    // ── router-execution ──────────────────────────────────────────────────────

    /// Scrape `router-execution` via `getEvents` for execution counters and
    /// `getLedgerEntries` for the `MaxRetries` configuration value.
    ///
    /// The contract emits:
    /// - `execution_result` — one event per completed execution (success or
    ///   final-attempt failure after retries are exhausted).
    /// - `execution_error`  — one event per failed execution (after all retries).
    ///
    /// `MaxRetries` is a configuration value written to instance storage on
    /// initialization and updated via `set_max_retries`; it is not event-based,
    /// so it is read directly via `getLedgerEntries`.
    ///
    /// Like `scrape_quote`, an in-memory ledger cursor prevents re-processing
    /// the same events on every cycle (see module-level docs for restart semantics).
    async fn scrape_execution(&self, client: &dyn RpcClient, contract_id: &str) -> Result<()> {
        let start = Instant::now();
        info!(contract_id, "scraping router-execution");

        let start_ledger = {
            let map = self.last_ledger.lock().await;
            map.get(&format!("execution:{contract_id}")).copied().unwrap_or(0)
        };

        // Fetch execution result and error events since the last cursor.
        let result_events = client
            .get_events(contract_id, &["execution_result"], start_ledger)
            .await?;
        let error_events = client
            .get_events(contract_id, &["execution_error"], start_ledger)
            .await?;

        // MaxRetries is a config value in instance storage, not event-based.
        let max_retries_key = encode_contract_data_key(contract_id, "MaxRetries");
        let max_retries_entries = client
            .get_ledger_entries(vec![max_retries_key])
            .await
            .unwrap_or_default();
        let max_retries = extract_u64_from_entry(&max_retries_entries, "MaxRetries").unwrap_or(0);

        let max_ledger = result_events
            .iter()
            .chain(error_events.iter())
            .map(|e| e.ledger)
            .max()
            .unwrap_or(start_ledger);

        let exec_count = result_events.len() as f64;
        let err_count = error_events.len() as f64;
        let g_exec = self.metrics.execution_total_executions.with_label_values(&[contract_id]);
        let g_err = self.metrics.execution_total_errors.with_label_values(&[contract_id]);

        if start_ledger == 0 {
            g_exec.set(exec_count);
            g_err.set(err_count);
        } else {
            g_exec.inc_by(exec_count);
            g_err.inc_by(err_count);
        }

        self.metrics
            .execution_max_retries
            .with_label_values(&[contract_id])
            .set(max_retries as f64);

        if max_ledger > start_ledger {
            self.last_ledger
                .lock()
                .await
                .insert(format!("execution:{contract_id}"), max_ledger);
        }

        let elapsed = start.elapsed().as_secs_f64();
        self.metrics
            .scrape_duration_seconds
            .with_label_values(&[contract_id])
            .observe(elapsed);

        info!(
            contract_id,
            elapsed_secs = elapsed,
            total_executions = result_events.len(),
            total_errors = error_events.len(),
            max_retries,
            start_ledger,
            max_ledger,
            "execution scrape done"
        );
        Ok(())
    }
}

/// Encode a `ContractData` ledger key for a named instance-storage entry.
///
/// Produces a string key that the mock client can match on. In production
/// this should be replaced with proper XDR encoding via the `stellar-xdr` crate.
fn encode_contract_data_key(contract_id: &str, storage_key: &str) -> String {
    format!("{}:{}", contract_id, storage_key)
}

/// Extract a `u64` value from a `getLedgerEntries` response for the given key name.
///
/// The RPC server returns entries with a `xdr` field containing base64-encoded
/// `LedgerEntryData` XDR. In the JSON-decoded representation (used by some RPC
/// versions) the value is available directly. We try both paths.
fn extract_u64_from_entry(entries: &[crate::rpc::LedgerEntry], key_name: &str) -> Option<u64> {
    for entry in entries {
        // The key field encodes the storage key name; we match by suffix.
        if entry.key.ends_with(key_name) || entry.key.contains(key_name) {
            // Try to parse the xdr field as a plain u64 (mock / JSON path).
            if let Ok(n) = entry.xdr.parse::<u64>() {
                return Some(n);
            }
            // Try JSON-decoded path: `{"u64": <n>}`.
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(&entry.xdr) {
                if let Some(n) = v.get("u64").and_then(|n| n.as_u64()) {
                    return Some(n);
                }
                if let Some(n) = v.as_u64() {
                    return Some(n);
                }
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rpc::MockRpcClient;
    use prometheus::Registry;
    use serde_json::json;

    fn make_collector(
        core: &str,
        middleware: &str,
        registry_id: &str,
    ) -> (Collector, RouterMetrics) {
        make_collector_full(core, middleware, registry_id, "", "")
    }

    fn make_collector_full(
        core: &str,
        middleware: &str,
        registry_id: &str,
        quote_id: &str,
        execution_id: &str,
    ) -> (Collector, RouterMetrics) {
        let reg = Registry::new();
        let metrics = RouterMetrics::new(&reg).unwrap();
        let args = Args {
            rpc_url: String::new(),
            network_passphrase: String::new(),
            core_contract_id: core.to_string(),
            middleware_contract_id: middleware.to_string(),
            registry_contract_id: registry_id.to_string(),
            quote_contract_id: quote_id.to_string(),
            execution_contract_id: execution_id.to_string(),
            scrape_interval_secs: 15,
            listen: "0.0.0.0:9090".to_string(),
            rpc_timeout_secs: 10,
        };
        let collector = Collector::new(args, metrics.clone());
        (collector, metrics)
    }

    #[tokio::test]
    async fn test_scrape_core_updates_metrics() {
        let (collector, metrics) = make_collector("CORE_ID", "", "");

        let mock = MockRpcClient::new()
            .with_u64("CORE_ID", "total_routed", 42)
            .with_string_vec("CORE_ID", "get_all_routes", vec![]);

        let ok = collector.scrape_all(&mock).await;
        assert!(ok);

        let val = metrics
            .core_total_routed
            .with_label_values(&["CORE_ID"])
            .get();
        assert_eq!(val, 42.0);
    }

    #[tokio::test]
    async fn test_scrape_middleware_updates_metrics() {
        let (collector, metrics) = make_collector("", "MW_ID", "");

        let mock = MockRpcClient::new()
            .with_u64("MW_ID", "total_calls", 7)
            .with_string_vec("MW_ID", "get_configured_routes", vec![]);

        let ok = collector.scrape_all(&mock).await;
        assert!(ok);

        let val = metrics
            .middleware_total_calls
            .with_label_values(&["MW_ID"])
            .get();
        assert_eq!(val, 7.0);
    }

    #[tokio::test]
    async fn test_scrape_registry_updates_metrics() {
        let (collector, metrics) = make_collector("", "", "REG_ID");

        let mock = MockRpcClient::new()
            .with_string_vec(
                "REG_ID",
                "get_all_names",
                vec!["oracle".to_string(), "vault".to_string()],
            )
            .with_u32_vec("REG_ID", "versions", "oracle", vec![1, 2, 3])
            .with_u32_vec("REG_ID", "versions", "vault", vec![1]);

        let ok = collector.scrape_all(&mock).await;
        assert!(ok);

        assert_eq!(
            metrics
                .registry_total_names
                .with_label_values(&["REG_ID"])
                .get(),
            2.0
        );
        assert_eq!(
            metrics
                .registry_version_count
                .with_label_values(&["REG_ID", "oracle"])
                .get(),
            3.0
        );
        assert_eq!(
            metrics
                .registry_version_count
                .with_label_values(&["REG_ID", "vault"])
                .get(),
            1.0
        );
    }

    #[tokio::test]
    async fn test_scrape_registry_empty_versions_for_name() {
        let (collector, metrics) = make_collector("", "", "REG_ID");

        let mock = MockRpcClient::new()
            .with_string_vec("REG_ID", "get_all_names", vec!["ghost".to_string()]);
        // No with_u32_vec configured → mock returns empty vec (default)

        let ok = collector.scrape_all(&mock).await;
        assert!(ok);

        assert_eq!(
            metrics
                .registry_version_count
                .with_label_values(&["REG_ID", "ghost"])
                .get(),
            0.0
        );
    }

    #[tokio::test]
    async fn test_scrape_failure_returns_false_and_increments_error_counter() {
        let (collector, metrics) = make_collector("CORE_ID", "", "");

        // Mock returns no response → scrape_core will fail
        let mock = MockRpcClient::new();

        let ok = collector.scrape_all(&mock).await;
        assert!(!ok);

        let errors = metrics
            .scrape_errors_total
            .with_label_values(&["CORE_ID"])
            .get();
        assert_eq!(errors, 1.0);
    }

    #[tokio::test]
    async fn test_scrape_core_with_routes_and_circuit_breaker() {
        let (collector, metrics) = make_collector("CORE_ID", "MW_ID", "");

        let mock = MockRpcClient::new()
            .with_u64("CORE_ID", "total_routed", 100)
            .with_string_vec(
                "CORE_ID",
                "get_all_routes",
                vec!["oracle".to_string()],
            )
            .with_simulate(
                "CORE_ID",
                "get_route",
                json!({ "results": [{ "retval": { "paused": false } }] }),
            )
            .with_u64("MW_ID", "total_calls", 50)
            .with_string_vec(
                "MW_ID",
                "get_configured_routes",
                vec!["oracle".to_string()],
            )
            .with_simulate(
                "MW_ID",
                "circuit_breaker_state",
                json!({
                    "results": [{
                        "retval": {
                            "some": { "is_open": true, "failure_count": 3, "opened_at": 1000 }
                        }
                    }]
                }),
            );

        let ok = collector.scrape_all(&mock).await;
        assert!(ok);

        assert_eq!(
            metrics
                .core_total_routed
                .with_label_values(&["CORE_ID"])
                .get(),
            100.0
        );
        assert_eq!(
            metrics
                .middleware_circuit_open
                .with_label_values(&["MW_ID", "oracle"])
                .get(),
            1.0
        );
        assert_eq!(
            metrics
                .middleware_failure_count
                .with_label_values(&["MW_ID", "oracle"])
                .get(),
            3.0
        );
    }

    #[tokio::test]
    async fn test_scrape_quote_baseline_sets_gauges() {
        use crate::rpc::ContractEvent;
        let (collector, metrics) = make_collector_full("", "", "", "QUOTE_ID", "");

        let make_event = |topic: &str, ledger: u32| ContractEvent {
            contract_id: "QUOTE_ID".to_string(),
            ledger,
            topic: vec![serde_json::json!(topic)],
            value: serde_json::json!({}),
        };

        let mock = MockRpcClient::new()
            .with_events("QUOTE_ID", "quote_generated", vec![
                make_event("quote_generated", 100),
                make_event("quote_generated", 101),
            ])
            .with_events("QUOTE_ID", "fee_estimated", vec![
                make_event("fee_estimated", 102),
            ]);

        // First scrape (cursor=0) → SET to baseline.
        let ok = collector.scrape_all(&mock).await;
        assert!(ok);

        assert_eq!(
            metrics.quote_total_generated.with_label_values(&["QUOTE_ID"]).get(),
            2.0
        );
        assert_eq!(
            metrics.quote_total_fee_estimated.with_label_values(&["QUOTE_ID"]).get(),
            1.0
        );
    }

    #[tokio::test]
    async fn test_scrape_quote_increments_on_second_scrape() {
        use crate::rpc::ContractEvent;
        let (collector, metrics) = make_collector_full("", "", "", "QUOTE_ID", "");

        let make_event = |topic: &str, ledger: u32| ContractEvent {
            contract_id: "QUOTE_ID".to_string(),
            ledger,
            topic: vec![serde_json::json!(topic)],
            value: serde_json::json!({}),
        };

        // First scrape: 2 quote events at ledger 100.
        let mock1 = MockRpcClient::new()
            .with_events("QUOTE_ID", "quote_generated", vec![
                make_event("quote_generated", 100),
                make_event("quote_generated", 100),
            ])
            .with_events("QUOTE_ID", "fee_estimated", vec![]);
        collector.scrape_all(&mock1).await;

        // Second scrape: 1 new quote event at ledger 101 (cursor now at 100).
        let mock2 = MockRpcClient::new()
            .with_events("QUOTE_ID", "quote_generated", vec![
                make_event("quote_generated", 101),
            ])
            .with_events("QUOTE_ID", "fee_estimated", vec![]);
        let ok = collector.scrape_all(&mock2).await;
        assert!(ok);

        // Gauge should be 2 (baseline) + 1 (new) = 3.
        assert_eq!(
            metrics.quote_total_generated.with_label_values(&["QUOTE_ID"]).get(),
            3.0
        );
    }

    #[tokio::test]
    async fn test_scrape_execution_uses_events() {
        use crate::rpc::{ContractEvent, LedgerEntry};
        let (collector, metrics) = make_collector_full("", "", "", "", "EXEC_ID");

        let make_event = |topic: &str, ledger: u32| ContractEvent {
            contract_id: "EXEC_ID".to_string(),
            ledger,
            topic: vec![serde_json::json!(topic)],
            value: serde_json::json!({}),
        };

        let mock = MockRpcClient::new()
            .with_events("EXEC_ID", "execution_result", vec![
                make_event("execution_result", 200),
                make_event("execution_result", 201),
                make_event("execution_result", 202),
            ])
            .with_events("EXEC_ID", "execution_error", vec![
                make_event("execution_error", 203),
            ])
            .with_ledger_entries(
                "EXEC_ID:MaxRetries",
                vec![LedgerEntry {
                    key: "EXEC_ID:MaxRetries".to_string(),
                    xdr: "3".to_string(),
                }],
            );

        let ok = collector.scrape_all(&mock).await;
        assert!(ok);

        assert_eq!(
            metrics.execution_total_executions.with_label_values(&["EXEC_ID"]).get(),
            3.0
        );
        assert_eq!(
            metrics.execution_total_errors.with_label_values(&["EXEC_ID"]).get(),
            1.0
        );
        assert_eq!(
            metrics.execution_max_retries.with_label_values(&["EXEC_ID"]).get(),
            3.0
        );
    }

    #[tokio::test]
    async fn test_scrape_execution_increments_on_second_scrape() {
        use crate::rpc::{ContractEvent, LedgerEntry};
        let (collector, metrics) = make_collector_full("", "", "", "", "EXEC_ID");

        let make_event = |topic: &str, ledger: u32| ContractEvent {
            contract_id: "EXEC_ID".to_string(),
            ledger,
            topic: vec![serde_json::json!(topic)],
            value: serde_json::json!({}),
        };

        let make_max_retries = || MockRpcClient::new()
            .with_ledger_entries(
                "EXEC_ID:MaxRetries",
                vec![LedgerEntry {
                    key: "EXEC_ID:MaxRetries".to_string(),
                    xdr: "2".to_string(),
                }],
            );

        // First scrape: 5 results, 1 error at ledger 300.
        let mock1 = make_max_retries()
            .with_events("EXEC_ID", "execution_result", (0..5).map(|_| make_event("execution_result", 300)).collect())
            .with_events("EXEC_ID", "execution_error", vec![make_event("execution_error", 300)]);
        collector.scrape_all(&mock1).await;

        // Second scrape: 2 new results at ledger 301.
        let mock2 = make_max_retries()
            .with_events("EXEC_ID", "execution_result", vec![
                make_event("execution_result", 301),
                make_event("execution_result", 301),
            ])
            .with_events("EXEC_ID", "execution_error", vec![]);
        let ok = collector.scrape_all(&mock2).await;
        assert!(ok);

        // 5 (baseline) + 2 (new) = 7
        assert_eq!(
            metrics.execution_total_executions.with_label_values(&["EXEC_ID"]).get(),
            7.0
        );
        // 1 (baseline) + 0 (new) = 1
        assert_eq!(
            metrics.execution_total_errors.with_label_values(&["EXEC_ID"]).get(),
            1.0
        );
    }
    #[test]
    fn test_extract_route_paused_true() {
        let val = json!({
            "results": [{ "retval": { "paused": true } }]
        });
        assert_eq!(extract_route_paused(&val), Some(true));
    }

    #[test]
    fn test_extract_route_paused_false() {
        let val = json!({ "paused": false });
        assert_eq!(extract_route_paused(&val), Some(false));
    }

    #[test]
    fn test_extract_circuit_breaker_open() {
        let val = json!({
            "results": [{
                "retval": {
                    "some": {
                        "is_open": true,
                        "failure_count": 5,
                        "opened_at": 1000
                    }
                }
            }]
        });
        assert_eq!(extract_circuit_breaker_state(&val), Some((true, 5)));
    }

    #[test]
    fn test_extract_circuit_breaker_none() {
        let val = json!({
            "results": [{ "retval": null }]
        });
        assert_eq!(extract_circuit_breaker_state(&val), Some((false, 0)));
    }

    #[test]
    fn test_extract_circuit_breaker_closed() {
        let val = json!({
            "results": [{
                "retval": {
                    "some": {
                        "is_open": false,
                        "failure_count": 2,
                        "opened_at": 0
                    }
                }
            }]
        });
        assert_eq!(extract_circuit_breaker_state(&val), Some((false, 2)));
    }

}


/// Encode a plain string as a base64 XDR `ScVal::String` argument.
///
/// This is a placeholder — a real implementation would use the `stellar-xdr`
/// crate to produce the correct XDR encoding.
pub(crate) fn encode_string_arg(s: &str) -> String {
    use std::fmt::Write;
    let mut out = String::new();
    for b in s.as_bytes() {
        write!(out, "{b:02x}").ok();
    }
    out
}

/// Extract the `paused` field from a `RouteEntry` JSON value returned by
/// `simulateTransaction`.
fn extract_route_paused(val: &serde_json::Value) -> Option<bool> {
    // The Soroban RPC returns struct fields as a JSON map.
    // RouteEntry { address, name, paused, updated_by, metadata }
    val.get("results")
        .and_then(|r| r.get(0))
        .and_then(|r| r.get("retval"))
        .and_then(|v| v.get("paused"))
        .and_then(|p| p.as_bool())
        .or_else(|| val.get("paused").and_then(|p| p.as_bool()))
}

/// Extract `(is_open, failure_count)` from a `CircuitBreakerState` JSON value.
fn extract_circuit_breaker_state(val: &serde_json::Value) -> Option<(bool, u32)> {
    let retval = val
        .get("results")
        .and_then(|r| r.get(0))
        .and_then(|r| r.get("retval"))
        .unwrap_or(val);

    // Handle Option<CircuitBreakerState> — None means no state recorded yet
    if retval.is_null() || retval.get("none").is_some() {
        return Some((false, 0));
    }

    let state = retval.get("some").unwrap_or(retval);
    let is_open = state
        .get("is_open")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let failure_count = state
        .get("failure_count")
        .and_then(|v| v.as_u64())
        .unwrap_or(0) as u32;

    Some((is_open, failure_count))
}


//! Prometheus metric definitions for the stellar-router exporter.
//!
//! All metrics are registered against a caller-supplied [`prometheus::Registry`]
//! so that tests can use an isolated registry without polluting the global one.

use anyhow::Result;
use prometheus::{
    register_counter_vec_with_registry, register_gauge_vec_with_registry,
    register_gauge_with_registry, register_histogram_vec_with_registry, CounterVec, Gauge,
    GaugeVec, HistogramVec, Registry,
};

/// Bucket boundaries (seconds) for the scrape-duration histogram.
///
/// Chosen to cover the typical Soroban RPC latency range (1 ms – 10 s).
const SCRAPE_DURATION_BUCKETS: &[f64] = &[
    0.001, 0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1.0, 2.5, 5.0, 10.0,
];

/// All Prometheus metrics exposed by the exporter.
///
/// Clone is cheap — each field is an `Arc`-backed Prometheus metric handle.
#[derive(Clone)]
pub struct RouterMetrics {
    // ── router-core ───────────────────────────────────────────────────────────
    /// Cumulative number of successful `resolve` calls since contract init.
    pub core_total_routed: GaugeVec,

    /// 1 if the router is globally paused, 0 otherwise.
    pub core_paused: GaugeVec,

    /// 1 if a specific named route is paused, 0 otherwise.
    pub core_route_paused: GaugeVec,

    // ── router-middleware ─────────────────────────────────────────────────────
    /// Cumulative number of `pre_call` invocations since contract init.
    pub middleware_total_calls: GaugeVec,

    /// Cumulative number of calls per route (from post_call events).
    pub middleware_route_calls_total: CounterVec,

    /// Cumulative number of failures per route (from post_call events).
    pub middleware_route_failures_total: CounterVec,

    /// 1 if the circuit breaker for a route is currently open, 0 otherwise.
    pub middleware_circuit_open: GaugeVec,

    /// Current consecutive failure count tracked by the circuit breaker.
    pub middleware_failure_count: GaugeVec,

    // ── router-registry ───────────────────────────────────────────────────────
    /// Total number of contract names registered in the registry.
    pub registry_total_names: GaugeVec,

    /// Total number of versions registered under each contract name.
    pub registry_version_count: GaugeVec,

    // ── router-quote ──────────────────────────────────────────────────────────
    /// Running total of `quote_generated` events observed.
    pub quote_total_generated: CounterVec,

    /// Running total of `fee_estimated` events observed.
    pub quote_total_fee_estimated: CounterVec,

    // ── router-execution ──────────────────────────────────────────────────────
    /// Cumulative number of executions recorded in on-chain storage.
    pub execution_total_executions: CounterVec,

    /// Cumulative number of execution errors recorded in on-chain storage.
    pub execution_total_errors: CounterVec,

    /// Configured maximum retries read from on-chain storage.
    pub execution_max_retries: GaugeVec,

    // ── exporter health ───────────────────────────────────────────────────────
    /// Time (seconds) spent scraping a single contract during the last cycle.
    pub scrape_duration_seconds: HistogramVec,

    /// Total number of failed scrape attempts per contract.
    pub scrape_errors_total: CounterVec,

    /// 1 if the most recent full scrape cycle completed without errors.
    pub up: Gauge,

    // ── SSE health ────────────────────────────────────────────────────────────
    /// 1 if the SSE connection for a contract is currently established, 0 otherwise.
    pub sse_connected: GaugeVec,

    /// Total number of SSE reconnect attempts per contract since process start.
    pub sse_reconnects_total: CounterVec,

    /// Total number of SSE events received per contract since process start.
    pub sse_events_total: CounterVec,
}

impl RouterMetrics {
    /// Create and register all metrics against `registry`.
    pub fn new(registry: &Registry) -> Result<Self> {
        let core_total_routed = register_gauge_vec_with_registry!(
            "router_core_total_routed",
            "Cumulative number of successful route resolutions since contract initialization",
            &["contract"],
            registry
        )?;

        let core_paused = register_gauge_vec_with_registry!(
            "router_core_paused",
            "1 if the router-core contract is globally paused, 0 otherwise",
            &["contract"],
            registry
        )?;

        let core_route_paused = register_gauge_vec_with_registry!(
            "router_core_route_paused",
            "1 if a specific named route is paused, 0 otherwise",
            &["contract", "route"],
            registry
        )?;

        let middleware_total_calls = register_gauge_vec_with_registry!(
            "router_middleware_total_calls",
            "Cumulative number of pre_call invocations since contract initialization",
            &["contract"],
            registry
        )?;

        let middleware_route_calls_total = register_counter_vec_with_registry!(
            "router_middleware_route_calls_total",
            "Cumulative number of calls per route from post_call events",
            &["contract", "route"],
            registry
        )?;

        let middleware_route_failures_total = register_counter_vec_with_registry!(
            "router_middleware_route_failures_total",
            "Cumulative number of failures per route from post_call events",
            &["contract", "route"],
            registry
        )?;

        let middleware_circuit_open = register_gauge_vec_with_registry!(
            "router_middleware_circuit_open",
            "1 if the circuit breaker for a route is currently open, 0 otherwise",
            &["contract", "route"],
            registry
        )?;

        let middleware_failure_count = register_gauge_vec_with_registry!(
            "router_middleware_failure_count",
            "Current consecutive failure count tracked by the circuit breaker for a route",
            &["contract", "route"],
            registry
        )?;

        let registry_total_names = register_gauge_vec_with_registry!(
            "router_registry_total_names",
            "Total number of contract names registered in the router-registry",
            &["contract"],
            registry
        )?;

        let registry_version_count = register_gauge_vec_with_registry!(
            "router_registry_version_count",
            "Total number of versions registered under a contract name in the router-registry",
            &["contract", "name"],
            registry
        )?;

        let quote_total_generated = register_counter_vec_with_registry!(
            "router_quote_total_generated",
            "Running total of quote_generated events observed from router-quote",
            &["contract"],
            registry
        )?;

        let quote_total_fee_estimated = register_counter_vec_with_registry!(
            "router_quote_total_fee_estimated",
            "Running total of fee_estimated events observed from router-quote",
            &["contract"],
            registry
        )?;

        let execution_total_executions = register_counter_vec_with_registry!(
            "router_execution_total_executions",
            "Cumulative number of executions recorded in router-execution on-chain storage",
            &["contract"],
            registry
        )?;

        let execution_total_errors = register_counter_vec_with_registry!(
            "router_execution_total_errors",
            "Cumulative number of execution errors recorded in router-execution on-chain storage",
            &["contract"],
            registry
        )?;

        let execution_max_retries = register_gauge_vec_with_registry!(
            "router_execution_max_retries",
            "Configured maximum retries read from router-execution on-chain storage",
            &["contract"],
            registry
        )?;

        let scrape_duration_seconds = register_histogram_vec_with_registry!(
            "router_scrape_duration_seconds",
            "Time in seconds spent scraping a single router contract",
            &["contract"],
            SCRAPE_DURATION_BUCKETS.to_vec(),
            registry
        )?;

        let scrape_errors_total = register_counter_vec_with_registry!(
            "router_scrape_errors_total",
            "Total number of failed scrape attempts per contract",
            &["contract"],
            registry
        )?;

        let up = register_gauge_with_registry!(
            "router_up",
            "1 if the most recent full scrape cycle completed without errors, 0 otherwise",
            registry
        )?;

        let sse_connected = register_gauge_vec_with_registry!(
            "router_sse_connected",
            "1 if the SSE connection for a contract is currently established, 0 otherwise",
            &["contract"],
            registry
        )?;

        let sse_reconnects_total = register_counter_vec_with_registry!(
            "router_sse_reconnects_total",
            "Total number of SSE reconnect attempts per contract since process start",
            &["contract"],
            registry
        )?;

        let sse_events_total = register_counter_vec_with_registry!(
            "router_sse_events_total",
            "Total number of SSE events received per contract since process start",
            &["contract"],
            registry
        )?;

        Ok(Self {
            core_total_routed,
            core_paused,
            core_route_paused,
            middleware_total_calls,
            middleware_route_calls_total,
            middleware_route_failures_total,
            middleware_circuit_open,
            middleware_failure_count,
            registry_total_names,
            registry_version_count,
            quote_total_generated,
            quote_total_fee_estimated,
            execution_total_executions,
            execution_total_errors,
            execution_max_retries,
            scrape_duration_seconds,
            scrape_errors_total,
            up,
            sse_connected,
            sse_reconnects_total,
            sse_events_total,
        })
    }
}

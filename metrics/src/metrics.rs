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

    // ── router-access ──────────────────────────────────────────────────────────
    /// Number of indexed members ever added for a given role (`get_role_count`).
    pub access_role_member_count: GaugeVec,

    /// Number of distinct addresses currently stored on the blacklist.
    pub access_blacklist_size: GaugeVec,

    // ── router-timelock ─────────────────────────────────────────────────────────
    /// Number of pending time-locked operations currently queued.
    pub timelock_pending_operations: GaugeVec,

    // ── router-multicall ────────────────────────────────────────────────────────
    /// Total number of batches submitted to the multicall contract (`total_batches`).
    pub multicall_total_batches: GaugeVec,

    /// Cumulative number of successful calls observed in `call_result` events.
    pub multicall_batch_success_total: CounterVec,

    /// Cumulative number of failed calls observed in `call_result` events.
    pub multicall_batch_failure_total: CounterVec,

    // ── exporter health ───────────────────────────────────────────────────────
    /// Time (seconds) spent scraping a single contract during the last cycle.
    pub scrape_duration_seconds: HistogramVec,

    /// Total number of failed scrape attempts per contract.
    pub scrape_errors_total: CounterVec,

    /// 1 if the most recent full scrape cycle completed without errors.
    pub up: Gauge,
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

        let access_role_member_count = register_gauge_vec_with_registry!(
            "router_access_role_member_count",
            "Number of indexed members ever added for a role in router-access (get_role_count)",
            &["contract", "role"],
            registry
        )?;

        let access_blacklist_size = register_gauge_vec_with_registry!(
            "router_access_blacklist_size",
            "Number of distinct addresses currently stored on the router-access blacklist",
            &["contract"],
            registry
        )?;

        let timelock_pending_operations = register_gauge_vec_with_registry!(
            "router_timelock_pending_operations",
            "Number of pending time-locked operations currently queued in router-timelock",
            &["contract"],
            registry
        )?;

        let multicall_total_batches = register_gauge_vec_with_registry!(
            "router_multicall_total_batches",
            "Total number of batches submitted to router-multicall (total_batches)",
            &["contract"],
            registry
        )?;

        let multicall_batch_success_total = register_counter_vec_with_registry!(
            "router_multicall_batch_success_total",
            "Cumulative number of successful calls observed in router-multicall call_result events",
            &["contract"],
            registry
        )?;

        let multicall_batch_failure_total = register_counter_vec_with_registry!(
            "router_multicall_batch_failure_total",
            "Cumulative number of failed calls observed in router-multicall call_result events",
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
            access_role_member_count,
            access_blacklist_size,
            timelock_pending_operations,
            multicall_total_batches,
            multicall_batch_success_total,
            multicall_batch_failure_total,
            scrape_duration_seconds,
            scrape_errors_total,
            up,
        })
    }
}

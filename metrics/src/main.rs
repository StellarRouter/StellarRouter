//! # router-metrics-exporter
//!
//! Off-chain Prometheus metrics exporter for the stellar-router suite.
//!
//! ## Overview
//!
//! Soroban smart contracts run inside the Stellar network as WASM and cannot
//! open sockets or push metrics themselves.  This binary bridges the gap:
//!
//! 1. It polls the Soroban RPC endpoint at a configurable interval.
//! 2. It reads on-chain state from each router contract (total_routed,
//!    total_calls, circuit-breaker state, paused flags, …).
//! 3. It exposes a `/metrics` HTTP endpoint in the Prometheus text format.
//!
//! ## Metrics exposed
//!
//! | Metric | Type | Labels | Description |
//! |--------|------|--------|-------------|
//! | `router_core_total_routed` | Gauge | `contract` | Cumulative successful route resolutions |
//! | `router_core_paused` | Gauge | `contract` | 1 if the router is globally paused |
//! | `router_core_route_paused` | Gauge | `contract`, `route` | 1 if a specific route is paused |
//! | `router_middleware_total_calls` | Gauge | `contract` | Cumulative pre-call invocations |
//! | `router_middleware_circuit_open` | Gauge | `contract`, `route` | 1 if the circuit breaker is open |
//! | `router_middleware_failure_count` | Gauge | `contract`, `route` | Consecutive failure count |
//! | `router_scrape_duration_seconds` | Histogram | `contract` | Time spent scraping each contract |
//! | `router_scrape_errors_total` | Counter | `contract` | Number of failed scrape attempts |
//! | `router_up` | Gauge | — | 1 if the last scrape cycle succeeded |

mod auth;
mod cli;
mod collector;
mod logging;
mod metrics;
mod openapi;
mod rate_limit;
mod replay_protection;
mod rpc;
mod server;
mod validation;

use anyhow::Result;
use clap::Parser;
use tracing::info;

use cli::Args;
use collector::Collector;
use logging::init_logging;
use metrics::RouterMetrics;
use rate_limit::{config_from_env, RateLimiter};
use server::serve;
use validation::{validate_contract_id, validate_listen_addr, validate_scrape_interval};

#[tokio::main]
async fn main() -> Result<()> {
    // ── Logging ───────────────────────────────────────────────────────────────
    init_logging("router_metrics_exporter=info")?;

    // ── CLI / env config ──────────────────────────────────────────────────────
    let args = Args::parse();

    // ── Input validation ──────────────────────────────────────────────────────
    validate_listen_addr(&args.listen)
        .map_err(|e| anyhow::anyhow!("invalid listen address: {}", e.message))?;
    validate_scrape_interval(args.scrape_interval_secs)
        .map_err(|e| anyhow::anyhow!("invalid scrape interval: {}", e.message))?;
    for id in [&args.core_contract_id, &args.middleware_contract_id, &args.registry_contract_id, &args.quote_contract_id, &args.execution_contract_id] {
        if !id.is_empty() {
            validate_contract_id(id)
                .map_err(|e| anyhow::anyhow!("invalid contract ID: {}", e.message))?;
        }
    }

    info!(
        rpc_url = %args.rpc_url,
        listen = %args.listen,
        scrape_interval_secs = args.scrape_interval_secs,
        "router-metrics-exporter starting"
    );

    // ── Prometheus registry ───────────────────────────────────────────────────
    let registry = prometheus::Registry::new();
    let router_metrics = RouterMetrics::new(&registry)?;

    // ── Background scrape loop ────────────────────────────────────────────────
    let collector = Collector::new(args.clone(), router_metrics.clone());
    tokio::spawn(async move {
        collector.run().await;
    });

    // ── HTTP server ───────────────────────────────────────────────────────────
    let limiter = RateLimiter::new(config_from_env());
    serve(args.listen, registry, limiter).await
}

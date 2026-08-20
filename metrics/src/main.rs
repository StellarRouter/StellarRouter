//! # router-metrics-exporter
//!
//! Off-chain Prometheus metrics exporter for the stellar-router suite.
//!
//! ## Overview
//!
//! Soroban smart contracts run inside the Stellar network as WASM and cannot
//! open sockets or push metrics themselves.  This binary bridges the gap:
//!
//! 1. It polls the Soroban RPC endpoint at a configurable interval (**poll mode**).
//! 2. Alternatively, it subscribes to the Stellar Horizon SSE event stream for
//!    near-real-time updates (**sse mode**).
//! 3. It reads on-chain state from each router contract (total_routed,
//!    total_calls, circuit-breaker state, paused flags, …).
//! 4. It exposes a `/metrics` HTTP endpoint in the Prometheus text format.
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
//! | `router_sse_connected` | Gauge | `contract` | 1 if SSE connection is active |
//! | `router_sse_reconnects_total` | Counter | `contract` | Total SSE reconnect attempts |
//! | `router_sse_events_total` | Counter | `contract` | Total SSE events received |

mod auth;
mod cli;
mod collector;
mod logging;
mod metrics;
mod openapi;
mod rate_limit;
mod replay_protection;
mod server;
mod sse;
mod validation;

use router_metrics_exporter::rpc;

use anyhow::Result;
use clap::Parser;
use tokio_util::sync::CancellationToken;
use tracing::info;

use cli::{Args, EventMode};
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
    for id in [
        &args.core_contract_id,
        &args.middleware_contract_id,
        &args.registry_contract_id,
        &args.quote_contract_id,
        &args.execution_contract_id,
    ] {
        if !id.is_empty() {
            validate_contract_id(id)
                .map_err(|e| anyhow::anyhow!("invalid contract ID: {}", e.message))?;
        }
    }

    info!(
        rpc_urls = ?if args.rpc_urls.is_empty() {
            vec![args.rpc_url.clone()]
        } else {
            args.rpc_urls.clone()
        },
        listen = %args.listen,
        scrape_interval_secs = args.scrape_interval_secs,
        event_mode = %args.event_mode,
        "router-metrics-exporter starting"
    );

    // ── Prometheus registry ───────────────────────────────────────────────────
    let registry = prometheus::Registry::new();
    let router_metrics = RouterMetrics::new(&registry)?;

    // ── Background scrape / SSE loop ──────────────────────────────────────────
    let collector = Collector::new(args.clone(), router_metrics.clone());

    match args.event_mode {
        EventMode::Poll => {
            info!("starting poll mode scrape loop");
            tokio::spawn(async move {
                collector.run().await;
            });
        }
        EventMode::Sse => {
            info!(
                horizon_url = %args.horizon_url,
                sse_max_reconnects = args.sse_max_reconnects,
                "starting SSE mode with bootstrap poll"
            );
            let cancel = CancellationToken::new();
            tokio::spawn(async move {
                collector.run_sse(cancel).await;
            });
        }
    }

    // ── HTTP server ───────────────────────────────────────────────────────────
    let limiter = RateLimiter::new(config_from_env());
    serve(args.listen, registry, limiter).await
}

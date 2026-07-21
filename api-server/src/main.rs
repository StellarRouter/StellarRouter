mod auth;
mod handlers;
mod openapi;
mod rate_limit;
mod replay_protection;
mod rpc;
mod state;
mod types;
mod websocket;
mod xdr;

#[cfg(test)]
mod tests;

use anyhow::{Context, Result};
use axum::{
    extract::DefaultBodyLimit,
    middleware::from_fn_with_state,
    routing::{get, post},
    Json, Router,
};
use utoipa::OpenApi;
use utoipa_swagger_ui::SwaggerUi;

use clap::Parser;
use std::net::SocketAddr;
use tracing::info;

use crate::{
    auth::AuthConfig,
    rate_limit::{rate_limit_middleware, RateLimitConfig, RateLimiter},
    replay_protection::{replay_protection_middleware, NonceCache, ReplayProtectionConfig},
    rpc::FeeConfig,
    state::AppState,
};

#[derive(Parser, Debug)]
#[command(name = "router-api-server")]
#[command(
    about = "API server for stellar-router with transaction simulation and WebSocket tracking"
)]
struct Args {
    /// Listen address (default: 127.0.0.1:8080)
    #[arg(long, env = "LISTEN_ADDR", default_value = "127.0.0.1:8080")]
    listen: String,

    /// Soroban RPC endpoint URL
    #[arg(long, env = "SOROBAN_RPC_URL")]
    rpc_url: String,

    /// Router execution contract ID
    #[arg(long, env = "ROUTER_EXECUTION_CONTRACT_ID")]
    execution_contract_id: String,

    /// Router core contract ID (for GET /routes)
    #[arg(long, env = "ROUTER_CORE_CONTRACT_ID", default_value = "")]
    router_core_contract_id: String,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive(tracing::Level::INFO.into()),
        )
        .init();

    let args = Args::parse();

    info!("Starting router-api-server");
    info!("Listen address: {}", args.listen);
    info!("RPC URL: {}", args.rpc_url);

    let auth_config = AuthConfig::from_env();
    info!("Router auth enabled: {}", auth_config.enabled);

    let rate_limit_config = RateLimitConfig::from_env()?;
    info!(
        max_requests = rate_limit_config.max_requests,
        window_secs = rate_limit_config.window.as_secs(),
        "Router API rate limiting enabled"
    );
    let rate_limiter = RateLimiter::new(rate_limit_config);

    let replay_config = ReplayProtectionConfig::from_env();
    info!(
        enabled = replay_config.enabled,
        cache_size = replay_config.cache_size,
        nonce_ttl_secs = replay_config.nonce_ttl_secs,
        "Router replay protection config loaded"
    );
    let nonce_cache = NonceCache::new(replay_config);

    let fee_config = FeeConfig::from_env();
    info!(
        base_fee = fee_config.base_fee,
        resource_fee_floor = fee_config.resource_fee_floor,
        resource_fee_divisor = fee_config.resource_fee_divisor,
        surge_load_threshold_bps = fee_config.surge_load_threshold_bps,
        surge_multiplier = fee_config.surge_multiplier,
        normal_multiplier = fee_config.normal_multiplier,
        "Router fee-estimation config loaded"
    );

    let state = AppState::new(
        args.rpc_url,
        args.execution_contract_id,
        args.router_core_contract_id,
        auth_config.clone(),
        fee_config,
    );

    let protected_routes = Router::new()
        .route("/simulate", post(handlers::simulate))
        .route_layer(from_fn_with_state(
            nonce_cache,
            replay_protection_middleware,
        ))
        .route("/routes", get(handlers::list_routes))
        .route("/routes/:name", get(handlers::get_route))
        .route("/ws", get(websocket::ws_handler))
        .route_layer(from_fn_with_state(rate_limiter, rate_limit_middleware))
        .route_layer(from_fn_with_state(auth_config, auth::auth_middleware));

    let app = Router::new()
        .route(
            "/api-docs/openapi.json",
            get(|| async { Json(openapi::ApiDoc::openapi()) }),
        )
        .merge(
            SwaggerUi::new("/swagger-ui").url("/api-docs/openapi.json", openapi::ApiDoc::openapi()),
        )
        .route("/health", get(handlers::health))
        .route("/stats", get(handlers::stats))
        .nest("/", protected_routes)
        .layer(DefaultBodyLimit::max(1024 * 1024))
        .with_state(state);

    let addr: SocketAddr = args
        .listen
        .parse()
        .with_context(|| format!("invalid listen address: {}", args.listen))?;

    info!("Server listening on {}", addr);

    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .await?;

    Ok(())
}

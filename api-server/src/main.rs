mod auth;
mod handlers;
mod rpc;
mod state;
mod types;
mod websocket;

#[cfg(test)]
mod tests;

use anyhow::{Context, Result};
use axum::{extract::DefaultBodyLimit, middleware::from_fn_with_state, routing::{get, post}, Router};
use clap::Parser;
use std::net::SocketAddr;
use tracing::info;

use crate::{auth::AuthConfig, state::AppState};

#[derive(Parser, Debug)]
#[command(name = "router-api-server")]
#[command(about = "API server for stellar-router with transaction simulation and WebSocket tracking")]
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

    let state = AppState::new(
        args.rpc_url,
        args.execution_contract_id,
        args.router_core_contract_id,
        auth_config.clone(),
    );

    let protected_routes = Router::new()
        .route("/simulate", post(handlers::simulate))
        .route("/routes", get(handlers::list_routes))
        .route("/routes/:name", get(handlers::get_route))
        .route("/ws", get(websocket::ws_handler))
        .route_layer(from_fn_with_state(auth_config, auth::auth_middleware));

    let app = Router::new()
        .route("/health", get(handlers::health))
        .nest("/", protected_routes)
        .layer(DefaultBodyLimit::max(1024 * 1024))
        .with_state(state);

    let addr: SocketAddr = args
        .listen
        .parse()
        .with_context(|| format!("invalid listen address: {}", args.listen))?;

    info!("Server listening on {}", addr);

    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app).await?;

    Ok(())
}

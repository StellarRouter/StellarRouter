mod handlers;
mod rpc;
mod state;
mod types;
mod websocket;

#[cfg(test)]
mod tests;

use anyhow::{Context, Result};
use axum::{
    extract::DefaultBodyLimit,
    routing::{get, post},
    Router,
};
use clap::Parser;
use std::net::SocketAddr;
use tracing::info;

use crate::state::AppState;

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

    let state = AppState::new(
        args.rpc_url,
        args.execution_contract_id,
        args.router_core_contract_id,
    );

    let app = Router::new()
        .route("/health", get(handlers::health))
        .route("/simulate", post(handlers::simulate))
        .route("/routes", get(handlers::list_routes))
        .route("/ws", get(websocket::ws_handler))
        .layer(DefaultBodyLimit::max(1024 * 1024)) // 1MB limit
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

use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
    Json,
};
use router_off_chain_common::validation::{validate_contract_id, validate_route_name};
use router_off_chain_common::logging::sanitize_for_log;
use router_off_chain_common::validation::{validate_contract_id, validate_route_name};
use router_off_chain_common::validation::{validate_contract_id, validate_function_name};
use serde_json::json;
use tracing::{error, info};

use crate::{
    state::AppState,
    types::{ErrorResponse, FeeEstimate, SimulateRequest, SimulateResponse, SimulationDetail, StatsResponse},
};

#[utoipa::path(
    get,
    path = "/health",
    responses(
        (status = 200, description = "Health check response")
    )
)]
/// GET /health
pub async fn health() -> impl IntoResponse {
    (StatusCode::OK, Json(json!({"status": "ok"})))
}

#[utoipa::path(
    get,
    path = "/stats",
    responses(
        (status = 200, description = "Server statistics", body = StatsResponse)
    )
)]
/// GET /stats
///
/// Returns live WebSocket connection and subscription statistics:
/// - `active_subscriptions`: total subscription count across all tx IDs
/// - `unique_tx_ids`: number of distinct tx IDs being tracked
/// - `broadcast_channel_capacity`: fixed capacity of the broadcast channel
pub async fn stats(State(state): State<AppState>) -> impl IntoResponse {
    Json(StatsResponse {
        active_subscriptions: state.active_subscriptions(),
        unique_tx_ids: state.unique_tx_ids(),
        broadcast_channel_capacity: state.broadcast_channel_capacity(),
    })
}

#[utoipa::path(
    post,
    path = "/simulate",
    request_body = SimulateRequest,
    responses(
        (status = 200, description = "Simulation result", body = SimulateResponse),
        (status = 400, description = "Bad request", body = ErrorResponse),
        (status = 503, description = "Service unavailable", body = ErrorResponse),
        (status = 500, description = "Internal server error", body = ErrorResponse)
    )
)]
/// POST /simulate
///
/// Calls the Soroban RPC `simulateTransaction` endpoint to get real fee
/// estimates. Falls back to heuristic estimates if the RPC is unavailable.
pub async fn simulate(
    State(state): State<AppState>,
    Json(req): Json<SimulateRequest>,
) -> Result<Json<SimulateResponse>, (StatusCode, Json<ErrorResponse>)> {
    // Use shared validation from router-off-chain-common
    if let Err(e) = validate_contract_id(&req.target) {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(ErrorResponse {
                error: format!(
                    "target must be a 56-character Stellar contract ID starting with C: {}",
                    e.message
                ),
            }),
        ));
    }

    if let Err(e) = validate_function_name(&req.function) {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(ErrorResponse {
                error: e.message,
            }),
        ));
    }

    info!(
        target = %req.target,
        function = %sanitize_for_log(&req.function),
        "simulating transaction"
    );

    let breakdown = state
        .rpc
        .simulate(&req.target, &req.function, req.amount, req.network_load_bps)
        .await
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ErrorResponse {
                    error: e.to_string(),
                }),
            )
        })?;

    Ok(Json(SimulateResponse {
        success: breakdown.would_succeed,
        estimated_fees: FeeEstimate {
            base_fee: breakdown.base_fee,
            resource_fee: breakdown.resource_fee,
            total_fee: breakdown.total_fee,
            surge_multiplier: breakdown.surge_multiplier,
            high_load: breakdown.high_load,
        },
        simulation: SimulationDetail {
            target: req.target,
            function: req.function,
            would_succeed: breakdown.would_succeed,
        },
        message: if breakdown.would_succeed {
            "Simulation successful".to_string()
        } else {
            "Simulation indicates transaction would fail".to_string()
        },
    }))
}

#[utoipa::path(
    get,
    path = "/routes/{name}",
    params(
        ("name" = String, Path, description = "Route name")
    ),
    responses(
        (status = 200, description = "Route entry", body = RouteEntryResponse),
        (status = 404, description = "Route not found", body = ErrorResponse),
        (status = 500, description = "Internal server error", body = ErrorResponse)
    )
)]
/// GET /routes/:name
///
/// Calls router-core::get_route(name) via the Soroban RPC and returns the
/// full RouteEntry as JSON. Returns 404 if the route does not exist.
pub async fn get_route(
    State(state): State<AppState>,
    Path(name): Path<String>,
) -> Result<impl IntoResponse, (StatusCode, Json<ErrorResponse>)> {
    // Validate before logging to prevent log injection via a malicious route
    // name containing newlines or other control characters.
    if let Err(e) = validate_route_name(&name) {
        return Err((
            StatusCode::UNPROCESSABLE_ENTITY,
            Json(ErrorResponse {
                error: format!("invalid route name: {}", e.message),
            }),
        ));
    }

    // `name` is now guaranteed to be alphanumeric/underscore/hyphen only —
    // safe to log directly.
    info!(route = %name, "fetching route");

    // Use shared validation from router-off-chain-common
    if let Err(e) = validate_route_name(&name) {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(ErrorResponse {
                error: format!("invalid route name: {}", e.message),
            }),
        ));
    }

    match state.rpc.get_route(&name).await {
        Ok(Some(entry)) => Ok((StatusCode::OK, Json(entry))),
        Ok(None) => Err((
            StatusCode::NOT_FOUND,
            Json(ErrorResponse {
                error: format!("route '{}' not found", name),
            }),
        )),
        Err(e) => Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse {
                error: e.to_string(),
            }),
        )),
    }
}

#[utoipa::path(
    get,
    path = "/routes",
    responses(
        (status = 200, description = "Routes list", body = serde_json::Value),
        (status = 503, description = "Service unavailable", body = ErrorResponse),
        (status = 500, description = "Internal server error", body = ErrorResponse)
    )
)]
/// GET /routes
///
/// Calls `get_all_routes` on the router-core contract via Soroban RPC and
/// returns the list of registered route names as JSON.
pub async fn list_routes(
    State(state): State<AppState>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    if state.router_core_contract_id.is_empty() {
        return Err((
            StatusCode::SERVICE_UNAVAILABLE,
            "ROUTER_CORE_CONTRACT_ID not configured".to_string(),
        ));
    }

    let routes = state
        .rpc
        .get_all_routes(&state.router_core_contract_id)
        .await
        .map_err(|e| {
            error!("Failed to fetch routes: {}", e);
            (StatusCode::INTERNAL_SERVER_ERROR, e.to_string())
        })?;

    info!("Returning {} routes", routes.len());
    Ok(Json(json!({ "routes": routes })))
}
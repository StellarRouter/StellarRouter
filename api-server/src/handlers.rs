use axum::{extract::State, http::StatusCode, response::IntoResponse, Json};
use serde_json::json;
use tracing::{error, info};

use crate::{
    rpc::SorobanRpcClient,
    state::AppState,
    types::{FeeEstimate, RouteBreakdown, RouteDetails, SimulateRequest, SimulateResponse},
};

/// GET /health
pub async fn health() -> impl IntoResponse {
    (StatusCode::OK, Json(json!({"status": "ok"})))
}

/// POST /simulate
///
/// Returns estimated fees and route breakdown without executing the transaction.
pub async fn simulate(
    State(state): State<AppState>,
    Json(req): Json<SimulateRequest>,
) -> Result<Json<SimulateResponse>, (StatusCode, String)> {
    info!(
        "Simulating transaction: target={}, function={}",
        req.target, req.function
    );

    if req.target.is_empty() || req.function.is_empty() {
        return Err((
            StatusCode::BAD_REQUEST,
            "target and function are required".to_string(),
        ));
    }

    let route_details = req.route_details.unwrap_or_else(|| RouteDetails {
        name: "default".to_string(),
        version: Some(1),
        expected_outputs: None,
    });

    let fee_estimate = FeeEstimate {
        base_fee: 100,
        resource_fee: 1000,
        total_fee: 1100,
        surge_multiplier: 100,
        high_load: false,
    };

    let expected_outputs = route_details
        .expected_outputs
        .unwrap_or_else(|| vec!["output_amount".to_string()]);

    let route_breakdown = RouteBreakdown {
        route_name: route_details.name.clone(),
        version: route_details.version.unwrap_or(1),
        target_contract: req.target.clone(),
        function: req.function.clone(),
    };

    Ok(Json(SimulateResponse {
        success: true,
        estimated_fees: fee_estimate,
        expected_outputs,
        route_breakdown,
        message: "Simulation successful".to_string(),
    }))
}

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

    let rpc = SorobanRpcClient::new(&state.rpc_url);
    let routes = rpc
        .get_all_routes(&state.router_core_contract_id)
        .await
        .map_err(|e| {
            error!("Failed to fetch routes: {}", e);
            (StatusCode::INTERNAL_SERVER_ERROR, e.to_string())
        })?;

    info!("Returning {} routes", routes.len());
    Ok(Json(json!({ "routes": routes })))
}

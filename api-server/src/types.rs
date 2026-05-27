use serde::{Deserialize, Serialize};

/// Transaction simulation request payload
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SimulateRequest {
    pub target: String,
    pub function: String,
    #[serde(default)]
    pub route_details: Option<RouteDetails>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RouteDetails {
    pub name: String,
    #[serde(default)]
    pub version: Option<u32>,
    #[serde(default)]
    pub expected_outputs: Option<Vec<String>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SimulateResponse {
    pub success: bool,
    pub estimated_fees: FeeEstimate,
    pub expected_outputs: Vec<String>,
    pub route_breakdown: RouteBreakdown,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FeeEstimate {
    pub base_fee: i64,
    pub resource_fee: i64,
    pub total_fee: i64,
    pub surge_multiplier: u32,
    pub high_load: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RouteBreakdown {
    pub route_name: String,
    pub version: u32,
    pub target_contract: String,
    pub function: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TransactionStatusEvent {
    pub tx_id: String,
    pub status: TransactionStatus,
    pub timestamp: String,
    #[serde(default)]
    pub message: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "UPPERCASE")]
pub enum TransactionStatus {
    Pending,
    Submitted,
    Confirmed,
    Failed,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubscribeMessage {
    pub action: String,
    pub tx_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WsMessage {
    pub msg_type: String,
    pub data: serde_json::Value,
}

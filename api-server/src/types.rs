use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SimulateRequest {
    /// Target contract address (56-char Stellar contract ID starting with C)
    pub target: String,
    /// Function name to invoke
    pub function: String,
    /// Transaction amount in stroops (used for fee estimation)
    #[serde(default = "default_amount")]
    pub amount: i64,
    /// Fee rate in basis points (default 30 = 0.30%)
    #[serde(default = "default_fee_bps")]
    pub fee_bps: u32,
    /// Network load in basis points for surge pricing (0–10000)
    #[serde(default)]
    pub network_load_bps: u32,
    #[serde(default)]
    pub route_details: Option<RouteDetails>,
}

fn default_amount() -> i64 {
    1_000_000
}

fn default_fee_bps() -> u32 {
    30
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
    pub simulation: SimulationDetail,
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
pub struct SimulationDetail {
    pub target: String,
    pub function: String,
    pub would_succeed: bool,
}

#[allow(dead_code)]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RouteBreakdown {
    pub route_name: String,
    pub version: u32,
    pub target_contract: String,
    pub function: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ErrorResponse {
    pub error: String,
}

/// Response for GET /routes/:name — mirrors router-core::RouteEntry
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RouteEntryResponse {
    pub address: String,
    pub name: String,
    pub paused: bool,
    pub updated_by: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<RouteMetadataResponse>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RouteMetadataResponse {
    pub description: String,
    pub tags: Vec<String>,
    pub owner: String,
}

/// Transaction status event (used by WebSocket)
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

#[allow(dead_code)]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WsMessage {
    pub msg_type: String,
    pub data: serde_json::Value,
}

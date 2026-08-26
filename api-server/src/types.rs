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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub route_breakdown: Option<RouteBreakdown>,
    pub message: String,
    /// Diagnostic events emitted during Soroban simulation.
    /// Omitted from JSON when empty (heuristic fallback path).
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub simulation_events: Vec<serde_json::Value>,
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

/// Response for GET /stats
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct StatsResponse {
    /// Total number of active WebSocket subscriptions (sum of all per-tx counts)
    pub active_subscriptions: usize,
    /// Number of distinct transaction IDs currently being tracked
    pub unique_tx_ids: usize,
    /// Broadcast channel capacity (fixed at startup)
    pub broadcast_channel_capacity: usize,
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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WsMessage {
    pub msg_type: String,
    pub data: serde_json::Value,
}

impl WsMessage {
    pub fn new(msg_type: impl Into<String>, data: serde_json::Value) -> Self {
        Self {
            msg_type: msg_type.into(),
            data,
        }
    }

    pub fn subscribed(tx_id: String) -> Self {
        Self::new(
            "subscribed",
            serde_json::json!({
                "tx_id": tx_id,
                "status": "subscribed",
            }),
        )
    }

    pub fn unsubscribed(tx_id: String) -> Self {
        Self::new("unsubscribed", serde_json::json!({ "tx_id": tx_id }))
    }

    pub fn status_update(event: &TransactionStatusEvent) -> Self {
        Self::new(
            "status_update",
            serde_json::json!({
                "tx_id": event.tx_id,
                "status": event.status,
                "timestamp": event.timestamp,
                "message": event.message,
            }),
        )
    }

    pub fn error(message: impl Into<String>) -> Self {
        Self::new(
            "error",
            serde_json::json!({
                "message": message.into()
            }),
        )
    }

    pub fn to_json_string(&self) -> String {
        serde_json::to_string(self).unwrap_or_else(|_| {
            r#"{"msg_type":"error","data":{"message":"Failed to serialize message"}}"#.to_string()
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ws_message_subscribed_creates_correct_structure() {
        let msg = WsMessage::subscribed("tx123".to_string());
        assert_eq!(msg.msg_type, "subscribed");
        assert_eq!(msg.data["tx_id"], "tx123");
        assert_eq!(msg.data["status"], "subscribed");
    }

    #[test]
    fn ws_message_unsubscribed_creates_correct_structure() {
        let msg = WsMessage::unsubscribed("tx456".to_string());
        assert_eq!(msg.msg_type, "unsubscribed");
        assert_eq!(msg.data["tx_id"], "tx456");
    }

    #[test]
    fn ws_message_status_update_creates_correct_structure() {
        let event = TransactionStatusEvent {
            tx_id: "tx789".to_string(),
            status: TransactionStatus::Confirmed,
            timestamp: "2024-01-01T00:00:00Z".to_string(),
            message: Some("Transaction confirmed".to_string()),
        };

        let msg = WsMessage::status_update(&event);
        assert_eq!(msg.msg_type, "status_update");
        assert_eq!(msg.data["tx_id"], "tx789");
        assert_eq!(msg.data["status"], "CONFIRMED");
        assert_eq!(msg.data["timestamp"], "2024-01-01T00:00:00Z");
        assert_eq!(msg.data["message"], "Transaction confirmed");
    }

    #[test]
    fn ws_message_error_creates_correct_structure() {
        let msg = WsMessage::error("Something went wrong");
        assert_eq!(msg.msg_type, "error");
        assert_eq!(msg.data["message"], "Something went wrong");
    }

    #[test]
    fn ws_message_serializes_to_valid_json() {
        let msg = WsMessage::new("test_type", serde_json::json!({"key": "value"}));
        let json_str = msg.to_json_string();

        let parsed: serde_json::Value = serde_json::from_str(&json_str).unwrap();
        assert_eq!(parsed["msg_type"], "test_type");
        assert_eq!(parsed["data"]["key"], "value");
    }

    #[test]
    fn ws_message_deserializes_from_json() {
        let json = r#"{"msg_type":"subscribed","data":{"tx_id":"tx999","status":"subscribed"}}"#;
        let msg: WsMessage = serde_json::from_str(json).unwrap();

        assert_eq!(msg.msg_type, "subscribed");
        assert_eq!(msg.data["tx_id"], "tx999");
        assert_eq!(msg.data["status"], "subscribed");
    }
}

/// Soroban RPC client for simulation, fee estimation, and contract reads.
///
/// Every `simulateTransaction` call now sends a properly-encoded
/// `TransactionEnvelope` XDR (v1, single `InvokeHostFunctionOp`). The
/// contract ID strkey is decoded to its 32-byte hash before encoding.
/// Response ScVal XDR is decoded with the typed parsers in `crate::xdr`.
use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};

use crate::{
    types::RouteEntryResponse,
    xdr::{self, ScArg},
};

#[derive(Debug, Clone)]
pub struct SorobanRpcClient {
    pub rpc_url: String,
    pub router_core_contract_id: Option<String>,
    http: reqwest::Client,
}

#[derive(Serialize)]
struct JsonRpcRequest<'a> {
    jsonrpc: &'a str,
    id: u64,
    method: &'a str,
    params: serde_json::Value,
}

#[derive(Deserialize, Debug)]
struct JsonRpcResponse<T> {
    result: Option<T>,
    error: Option<JsonRpcError>,
}

#[derive(Deserialize, Debug)]
struct JsonRpcError {
    message: String,
}

#[derive(Deserialize, Debug)]
pub struct SimulateTransactionResult {
    #[serde(rename = "minResourceFee", default)]
    pub min_resource_fee: String,
    pub error: Option<String>,
    #[serde(default)]
    pub events: Vec<serde_json::Value>,
}

#[derive(Deserialize, Debug)]
struct SimulateTransactionResultWithReturnValue {
    #[serde(rename = "minResourceFee", default)]
    pub min_resource_fee: String,
    pub error: Option<String>,
    #[serde(default)]
    pub results: Vec<InvokeResult>,
}

#[derive(Deserialize, Debug)]
struct InvokeResult {
    /// Base64-encoded XDR of the `ScVal` return value.
    pub xdr: String,
}

#[derive(Debug)]
pub struct FeeBreakdown {
    pub base_fee: i64,
    pub resource_fee: i64,
    pub total_fee: i64,
    pub surge_multiplier: u32,
    pub high_load: bool,
    pub would_succeed: bool,
}

impl SorobanRpcClient {
    pub fn new(rpc_url: impl Into<String>, router_core_contract_id: Option<String>) -> Self {
        Self {
            rpc_url: rpc_url.into(),
            router_core_contract_id,
            http: reqwest::Client::new(),
        }
    }

    pub async fn simulate(
        &self,
        target: &str,
        function: &str,
        amount: i64,
        network_load_bps: u32,
    ) -> Result<FeeBreakdown> {
        match self.call_simulate_rpc(target, function).await {
            Ok(result) => {
                let would_succeed = result.error.is_none();
                let resource_fee: i64 = result.min_resource_fee.parse().unwrap_or(1_000);
                let base_fee: i64 = 100;
                let (surge_multiplier, high_load) = if network_load_bps >= 8_000 {
                    (200u32, true)
                } else {
                    (100u32, false)
                };
                let total_fee = (base_fee + resource_fee) * surge_multiplier as i64 / 100;
                Ok(FeeBreakdown {
                    base_fee,
                    resource_fee,
                    total_fee,
                    surge_multiplier,
                    high_load,
                    would_succeed,
                })
            }
            Err(_) => Ok(Self::heuristic_estimate(amount, network_load_bps)),
        }
    }

    /// Fetch all registered route names from `router-core::get_all_routes()`.
    ///
    /// Sends a valid `simulateTransaction` XDR and decodes the `ScVal::Vec`
    /// return value. Returns an empty list on RPC error rather than failing
    /// the endpoint, consistent with the heuristic fallback in `simulate`.
    pub async fn get_all_routes(&self, contract_id: &str) -> Result<Vec<String>> {
        let hash = xdr::decode_contract_id(contract_id)
            .map_err(|e| anyhow!("invalid ROUTER_CORE_CONTRACT_ID: {}", e))?;

        let tx_xdr = xdr::build_invoke_xdr(&hash, "get_all_routes", &[]);

        let req = JsonRpcRequest {
            jsonrpc: "2.0",
            id: 1,
            method: "simulateTransaction",
            params: serde_json::json!({ "transaction": tx_xdr }),
        };

        let resp: JsonRpcResponse<SimulateTransactionResultWithReturnValue> = self
            .http
            .post(&self.rpc_url)
            .json(&req)
            .send()
            .await
            .map_err(|e| anyhow!("RPC request failed: {}", e))?
            .json()
            .await
            .map_err(|e| anyhow!("Failed to parse RPC response: {}", e))?;

        if let Some(err) = resp.error {
            return Err(anyhow!("RPC error: {}", err.message));
        }

        let result = resp.result.ok_or_else(|| anyhow!("empty RPC result"))?;

        if let Some(err) = result.error {
            return Err(anyhow!("contract error: {}", err));
        }

        let routes = result
            .results
            .into_iter()
            .next()
            .map(|r| xdr::parse_string_vec(&r.xdr))
            .transpose()?
            .unwrap_or_default();

        Ok(routes)
    }

    /// Fetch a single route entry from `router-core::get_route(name)`.
    ///
    /// Sends a valid `simulateTransaction` XDR with the route name encoded as
    /// an `ScVal::String` argument. The `ScVal::Map` return value is decoded
    /// into a `RouteEntryResponse`. Returns `Ok(None)` when the contract
    /// returns `ScVal::Void` (route not found).
    pub async fn get_route(&self, name: &str) -> Result<Option<RouteEntryResponse>> {
        let contract_id = self
            .router_core_contract_id
            .as_deref()
            .ok_or_else(|| anyhow!("ROUTER_CORE_CONTRACT_ID not configured"))?;

        let hash = xdr::decode_contract_id(contract_id)
            .map_err(|e| anyhow!("invalid ROUTER_CORE_CONTRACT_ID: {}", e))?;

        let tx_xdr = xdr::build_invoke_xdr(&hash, "get_route", &[ScArg::String(name)]);

        let req = JsonRpcRequest {
            jsonrpc: "2.0",
            id: 1,
            method: "simulateTransaction",
            params: serde_json::json!({
                "transaction": tx_xdr,
                "resourceConfig": { "instructionLeeway": 3_000_000 }
            }),
        };

        let resp: JsonRpcResponse<SimulateTransactionResultWithReturnValue> = self
            .http
            .post(&self.rpc_url)
            .json(&req)
            .send()
            .await
            .map_err(|e| anyhow!("RPC request failed: {}", e))?
            .json()
            .await
            .map_err(|e| anyhow!("Failed to parse RPC response: {}", e))?;

        if let Some(err) = resp.error {
            return Err(anyhow!("RPC error: {}", err.message));
        }

        let result = match resp.result {
            Some(r) => r,
            None => return Ok(None),
        };

        if let Some(err) = result.error {
            return Err(anyhow!("contract error: {}", err));
        }

        let xdr_b64 = match result.results.into_iter().next() {
            Some(r) => r.xdr,
            None => return Ok(None),
        };

        let entry = match xdr::parse_route_entry(&xdr_b64)? {
            Some(e) => e,
            None => return Ok(None),
        };

        Ok(Some(RouteEntryResponse {
            address: entry.address,
            name: entry.name,
            paused: entry.paused,
            updated_by: entry.updated_by,
            // Metadata is stored separately in router-core (DataKey::Metadata)
            // and would require a second getLedgerEntries call to retrieve.
            metadata: None,
        }))
    }

    /// Call `simulateTransaction` for fee estimation.
    ///
    /// Encodes a valid `InvokeHostFunctionOp` XDR so the Soroban RPC can
    /// return real resource-fee data. Falls back to `heuristic_estimate`
    /// in `simulate()` when this call fails.
    async fn call_simulate_rpc(
        &self,
        target: &str,
        function: &str,
    ) -> Result<SimulateTransactionResult> {
        let hash = xdr::decode_contract_id(target)
            .map_err(|e| anyhow!("invalid contract ID '{}': {}", target, e))?;

        let tx_xdr = xdr::build_invoke_xdr(&hash, function, &[]);

        let req = JsonRpcRequest {
            jsonrpc: "2.0",
            id: 1,
            method: "simulateTransaction",
            params: serde_json::json!({ "transaction": tx_xdr }),
        };
        let resp: JsonRpcResponse<SimulateTransactionResult> = self
            .http
            .post(&self.rpc_url)
            .json(&req)
            .send()
            .await?
            .json()
            .await?;
        if let Some(err) = resp.error {
            return Err(anyhow!("RPC error: {}", err.message));
        }
        resp.result.ok_or_else(|| anyhow!("empty RPC result"))
    }

    fn heuristic_estimate(amount: i64, network_load_bps: u32) -> FeeBreakdown {
        let base_fee: i64 = 100;
        let resource_fee: i64 = {
            let scaled = amount / 1_000;
            if scaled < 100 { 100 } else { scaled }
        };
        let (surge_multiplier, high_load) = if network_load_bps >= 8_000 {
            (200u32, true)
        } else {
            (100u32, false)
        };
        let total_fee = (base_fee + resource_fee) * surge_multiplier as i64 / 100;
        FeeBreakdown {
            base_fee,
            resource_fee,
            total_fee,
            surge_multiplier,
            high_load,
            would_succeed: true,
        }
    }
}

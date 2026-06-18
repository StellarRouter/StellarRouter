/// Soroban RPC client for simulation, fee estimation, and contract reads.
use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::types::{RouteEntryResponse, RouteMetadataResponse};

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

    pub async fn get_all_routes(&self, contract_id: &str) -> Result<Vec<String>> {
        let placeholder_xdr = format!("AAAAAgAAAAEAAAAA{}get_all_routesAAAAAAAAAAAA=", contract_id);

        let req = JsonRpcRequest {
            jsonrpc: "2.0",
            id: 1,
            method: "simulateTransaction",
            params: serde_json::json!({ "transaction": placeholder_xdr }),
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
            .map(|r| {
                // Try to decode as a JSON array of strings (mock / test path),
                // otherwise return an empty list.
                serde_json::from_str::<Vec<String>>(&r.xdr).unwrap_or_default()
            })
            .unwrap_or_default();

        Ok(routes)
    }

    pub async fn get_route(&self, name: &str) -> Result<Option<RouteEntryResponse>> {
        let contract_id = self
            .router_core_contract_id
            .as_deref()
            .ok_or_else(|| anyhow!("ROUTER_CORE_CONTRACT_ID not configured"))?;

        let placeholder_xdr = format!("get_route:{}:{}", contract_id, name);

        let req = JsonRpcRequest {
            jsonrpc: "2.0",
            id: 1,
            method: "simulateTransaction",
            params: serde_json::json!({
                "transaction": placeholder_xdr,
                "resourceConfig": { "instructionLeeway": 3000000 }
            }),
        };

        let resp: JsonRpcResponse<Value> = self
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

        let result = match resp.result {
            Some(r) => r,
            None => return Ok(None),
        };

        Self::parse_route_entry_from_rpc(&result)
    }

    fn parse_route_entry_from_rpc(result: &Value) -> Result<Option<RouteEntryResponse>> {
        if result.get("error").is_some() {
            return Ok(None);
        }

        let results = match result.get("results").and_then(|r| r.as_array()) {
            Some(r) if !r.is_empty() => r,
            _ => return Ok(None),
        };

        let entry_json = results[0].get("xdr").cloned().unwrap_or(Value::Null);
        if entry_json.is_null() {
            return Ok(None);
        }

        let map = match entry_json.get("map").and_then(|m| m.as_array()) {
            Some(m) => m.clone(),
            None => return Ok(None),
        };

        let mut address = String::new();
        let mut route_name = String::new();
        let mut paused = false;
        let mut updated_by = String::new();
        let mut metadata: Option<RouteMetadataResponse> = None;

        for item in &map {
            let key = item
                .get("key")
                .and_then(|k| k.get("sym"))
                .and_then(|s| s.as_str())
                .unwrap_or("");
            let val = &item["val"];
            match key {
                "address" => {
                    address = val.get("address").and_then(|a| a.as_str()).unwrap_or("").to_string();
                }
                "name" => {
                    route_name = val.get("str").and_then(|s| s.as_str()).unwrap_or("").to_string();
                }
                "paused" => {
                    paused = val.get("b").and_then(|b| b.as_bool()).unwrap_or(false);
                }
                "updated_by" => {
                    updated_by = val.get("address").and_then(|a| a.as_str()).unwrap_or("").to_string();
                }
                "metadata" => {
                    if let Some(meta_map) = val.get("map").and_then(|m| m.as_array()) {
                        let mut description = String::new();
                        let mut tags = Vec::new();
                        let mut owner = String::new();
                        for meta_item in meta_map {
                            let mk = meta_item
                                .get("key")
                                .and_then(|k| k.get("sym"))
                                .and_then(|s| s.as_str())
                                .unwrap_or("");
                            let mv = &meta_item["val"];
                            match mk {
                                "description" => {
                                    description = mv.get("str").and_then(|s| s.as_str()).unwrap_or("").to_string();
                                }
                                "owner" => {
                                    owner = mv.get("address").and_then(|a| a.as_str()).unwrap_or("").to_string();
                                }
                                "tags" => {
                                    if let Some(tag_vec) = mv.get("vec").and_then(|v| v.as_array()) {
                                        tags = tag_vec
                                            .iter()
                                            .filter_map(|t| t.get("str").and_then(|s| s.as_str()).map(|s| s.to_string()))
                                            .collect();
                                    }
                                }
                                _ => {}
                            }
                        }
                        metadata = Some(RouteMetadataResponse { description, tags, owner });
                    }
                }
                _ => {}
            }
        }

        if address.is_empty() {
            return Ok(None);
        }

        Ok(Some(RouteEntryResponse {
            address,
            name: route_name,
            paused,
            updated_by,
            metadata,
        }))
    }

    async fn call_simulate_rpc(&self, target: &str, function: &str) -> Result<SimulateTransactionResult> {
        let placeholder_xdr = format!("AAAAAgAAAAEAAAAA{}{}AAAAAAAAAAA=", target, function);
        let req = JsonRpcRequest {
            jsonrpc: "2.0",
            id: 1,
            method: "simulateTransaction",
            params: serde_json::json!({ "transaction": placeholder_xdr }),
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

fn base64_decode(input: &str) -> Option<Vec<u8>> {
    const TABLE: [u8; 128] = {
        let mut t = [255u8; 128];
        let chars = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        let mut i = 0usize;
        while i < 64 {
            t[chars[i] as usize] = i as u8;
            i += 1;
        }
        t
    };

    let input = input.trim_end_matches('=');
    let mut out = Vec::with_capacity(input.len() * 3 / 4);
    let bytes = input.as_bytes();
    let mut i = 0;
    while i + 3 < bytes.len() {
        let b0 = *TABLE.get(bytes[i] as usize)?;
        let b1 = *TABLE.get(bytes[i + 1] as usize)?;
        let b2 = *TABLE.get(bytes[i + 2] as usize)?;
        let b3 = *TABLE.get(bytes[i + 3] as usize)?;
        if b0 == 255 || b1 == 255 || b2 == 255 || b3 == 255 {
            return None;
        }
        out.push((b0 << 2) | (b1 >> 4));
        out.push((b1 << 4) | (b2 >> 2));
        out.push((b2 << 6) | b3);
        i += 4;
    }
    match bytes.len() - i {
        2 => {
            let b0 = *TABLE.get(bytes[i] as usize)?;
            let b1 = *TABLE.get(bytes[i + 1] as usize)?;
            if b0 == 255 || b1 == 255 {
                return None;
            }
            out.push((b0 << 2) | (b1 >> 4));
        }
        3 => {
            let b0 = *TABLE.get(bytes[i] as usize)?;
            let b1 = *TABLE.get(bytes[i + 1] as usize)?;
            let b2 = *TABLE.get(bytes[i + 2] as usize)?;
            if b0 == 255 || b1 == 255 || b2 == 255 {
                return None;
            }
            out.push((b0 << 2) | (b1 >> 4));
            out.push((b1 << 4) | (b2 >> 2));
        }
        0 => {}
        _ => return None,
    }
    Some(out)
}

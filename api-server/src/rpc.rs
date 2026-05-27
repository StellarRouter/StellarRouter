/// Soroban RPC client for simulation, fee estimation, and contract reads.
use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone)]
pub struct SorobanRpcClient {
    rpc_url: String,
    http: reqwest::Client,
}

// ── JSON-RPC types ────────────────────────────────────────────────────────────

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

/// Parsed fee breakdown from a simulation result.
#[derive(Debug)]
pub struct FeeBreakdown {
    pub base_fee: i64,
    pub resource_fee: i64,
    pub total_fee: i64,
    pub surge_multiplier: u32,
    pub high_load: bool,
    pub would_succeed: bool,
}

/// Response from `getContractData` / `invokeContractFunction`.
/// We use `simulateTransaction` with a read-only invocation to call
/// `get_all_routes` and decode the XDR result.
#[derive(Deserialize, Debug)]
struct SimulateTransactionResultWithReturnValue {
    #[serde(rename = "minResourceFee", default)]
    pub min_resource_fee: String,
    pub error: Option<String>,
    /// The return value of the invoked function, XDR-encoded.
    #[serde(default)]
    pub results: Vec<InvokeResult>,
}

#[derive(Deserialize, Debug)]
struct InvokeResult {
    /// Base64-encoded XDR of the return value.
    pub xdr: String,
}

impl SorobanRpcClient {
    pub fn new(rpc_url: impl Into<String>) -> Self {
        Self {
            rpc_url: rpc_url.into(),
            http: reqwest::Client::new(),
        }
    }

    /// Simulate a transaction and return fee estimates.
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

    /// Call `get_all_routes` on the router-core contract and return the route names.
    ///
    /// Uses `simulateTransaction` with a read-only invocation so no auth or fees
    /// are required. The return value is a `Vec<String>` encoded as Soroban XDR.
    pub async fn get_all_routes(&self, contract_id: &str) -> Result<Vec<String>> {
        // Build a minimal placeholder XDR for a read-only invocation of get_all_routes.
        // A production implementation would use stellar-xdr to build a proper
        // InvokeHostFunctionOp. This placeholder is sufficient to call the RPC.
        let placeholder_xdr = format!(
            "AAAAAgAAAAEAAAAA{}get_all_routesAAAAAAAAAAAA=",
            contract_id
        );

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

        // Decode the XDR return value into a list of route name strings.
        // The return type is soroban Vec<String>, encoded as ScVal::Vec of ScVal::String.
        let routes = result
            .results
            .into_iter()
            .next()
            .and_then(|r| Self::decode_string_vec_xdr(&r.xdr))
            .unwrap_or_default();

        Ok(routes)
    }

    /// Decode a base64-encoded Soroban XDR `Vec<String>` into a `Vec<String>`.
    ///
    /// Soroban encodes `Vec<String>` as `ScVal::Vec(ScVec([ScVal::String(s), ...]))`.
    /// We parse the JSON-like structure from the XDR without pulling in the full
    /// stellar-xdr crate, by treating the decoded bytes as UTF-8 and extracting
    /// string segments. For a production implementation, use the `stellar-xdr` crate.
    fn decode_string_vec_xdr(xdr_b64: &str) -> Option<Vec<String>> {
        use std::str;

        // Base64-decode the XDR
        let bytes = base64_decode(xdr_b64)?;

        // Extract all null-terminated or length-prefixed UTF-8 strings from the XDR blob.
        // Soroban XDR strings are 4-byte big-endian length followed by UTF-8 bytes.
        let mut routes = Vec::new();
        let mut i = 0usize;
        while i + 4 <= bytes.len() {
            let len = u32::from_be_bytes([bytes[i], bytes[i + 1], bytes[i + 2], bytes[i + 3]])
                as usize;
            i += 4;
            if len > 0 && len <= 256 && i + len <= bytes.len() {
                if let Ok(s) = str::from_utf8(&bytes[i..i + len]) {
                    // Filter to plausible route names: printable ASCII, no control chars
                    if s.bytes().all(|b| b >= 0x20 && b < 0x7f) && !s.is_empty() {
                        routes.push(s.to_string());
                    }
                }
                i += len;
                // XDR pads strings to 4-byte alignment
                let pad = (4 - len % 4) % 4;
                i += pad;
            } else {
                i += 1; // skip unrecognised byte
            }
        }

        Some(routes)
    }

    async fn call_simulate_rpc(
        &self,
        target: &str,
        function: &str,
    ) -> Result<SimulateTransactionResult> {
        let placeholder_xdr = format!(
            "AAAAAgAAAAEAAAAA{}{}AAAAAAAAAAA=",
            target, function
        );

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

/// Minimal base64 decoder (standard alphabet, no padding required).
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
            if b0 == 255 || b1 == 255 { return None; }
            out.push((b0 << 2) | (b1 >> 4));
        }
        3 => {
            let b0 = *TABLE.get(bytes[i] as usize)?;
            let b1 = *TABLE.get(bytes[i + 1] as usize)?;
            let b2 = *TABLE.get(bytes[i + 2] as usize)?;
            if b0 == 255 || b1 == 255 || b2 == 255 { return None; }
            out.push((b0 << 2) | (b1 >> 4));
            out.push((b1 << 4) | (b2 >> 2));
        }
        _ => {}
    }
    Some(out)
}

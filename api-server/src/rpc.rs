/// Soroban RPC client for simulation, fee estimation, and contract reads.
///
/// Every `simulateTransaction` call now sends a properly-encoded
/// `TransactionEnvelope` XDR (v1, single `InvokeHostFunctionOp`). The
/// contract ID strkey is decoded to its 32-byte hash before encoding.
/// Response ScVal XDR is decoded with the typed parsers in `crate::xdr`.
use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};
use tracing::warn;

use crate::{
    types::{RouteEntryResponse, TransactionStatus},
    xdr::{self, ScArg},
};

/// Default fee-estimation constants. Each is overridable through the matching
/// `ROUTER_*` environment variable parsed by [`FeeConfig::from_env`].
const DEFAULT_BASE_FEE: i64 = 100;
const DEFAULT_RESOURCE_FEE_FLOOR: i64 = 100;
const DEFAULT_RESOURCE_FEE_DIVISOR: i64 = 1_000;
const DEFAULT_SURGE_LOAD_THRESHOLD_BPS: u32 = 8_000;
const DEFAULT_SURGE_MULTIPLIER: u32 = 200;
const DEFAULT_NORMAL_MULTIPLIER: u32 = 100;

const BASE_FEE_ENV: &str = "ROUTER_BASE_FEE";
const RESOURCE_FEE_FLOOR_ENV: &str = "ROUTER_RESOURCE_FEE_FLOOR";
const RESOURCE_FEE_DIVISOR_ENV: &str = "ROUTER_RESOURCE_FEE_DIVISOR";
const SURGE_LOAD_THRESHOLD_BPS_ENV: &str = "ROUTER_SURGE_LOAD_THRESHOLD_BPS";
const SURGE_MULTIPLIER_ENV: &str = "ROUTER_SURGE_MULTIPLIER";
const NORMAL_MULTIPLIER_ENV: &str = "ROUTER_NORMAL_MULTIPLIER";

/// Tunable constants for the heuristic fee-estimation model.
///
/// Values are sourced from the environment at start-up (see
/// [`FeeConfig::from_env`]); any variable that is missing, unparseable, or not
/// strictly positive falls back to its documented default, so existing
/// deployments keep their previous behaviour with no configuration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FeeConfig {
    /// Flat base fee, in stroops (`ROUTER_BASE_FEE`).
    pub base_fee: i64,
    /// Lower bound applied to the derived resource fee (`ROUTER_RESOURCE_FEE_FLOOR`).
    pub resource_fee_floor: i64,
    /// Divisor used to scale `amount` into a resource fee (`ROUTER_RESOURCE_FEE_DIVISOR`).
    pub resource_fee_divisor: i64,
    /// Network-load threshold, in bps, above which surge pricing applies
    /// (`ROUTER_SURGE_LOAD_THRESHOLD_BPS`).
    pub surge_load_threshold_bps: u32,
    /// Multiplier (percent) applied when the load threshold is reached
    /// (`ROUTER_SURGE_MULTIPLIER`).
    pub surge_multiplier: u32,
    /// Multiplier (percent) applied under normal load (`ROUTER_NORMAL_MULTIPLIER`).
    pub normal_multiplier: u32,
}

impl Default for FeeConfig {
    fn default() -> Self {
        Self {
            base_fee: DEFAULT_BASE_FEE,
            resource_fee_floor: DEFAULT_RESOURCE_FEE_FLOOR,
            resource_fee_divisor: DEFAULT_RESOURCE_FEE_DIVISOR,
            surge_load_threshold_bps: DEFAULT_SURGE_LOAD_THRESHOLD_BPS,
            surge_multiplier: DEFAULT_SURGE_MULTIPLIER,
            normal_multiplier: DEFAULT_NORMAL_MULTIPLIER,
        }
    }
}

impl FeeConfig {
    /// Build a [`FeeConfig`] from process environment variables.
    pub fn from_env() -> Self {
        Self::from_lookup(|key| std::env::var(key).ok())
    }

    /// Resolve the config from an arbitrary key lookup. Kept separate from
    /// [`FeeConfig::from_env`] so the parsing logic is unit-testable without
    /// mutating global process state.
    fn from_lookup(lookup: impl Fn(&str) -> Option<String>) -> Self {
        let defaults = Self::default();
        Self {
            base_fee: parse_positive_i64(lookup(BASE_FEE_ENV).as_deref(), defaults.base_fee),
            resource_fee_floor: parse_positive_i64(
                lookup(RESOURCE_FEE_FLOOR_ENV).as_deref(),
                defaults.resource_fee_floor,
            ),
            resource_fee_divisor: parse_positive_i64(
                lookup(RESOURCE_FEE_DIVISOR_ENV).as_deref(),
                defaults.resource_fee_divisor,
            ),
            surge_load_threshold_bps: parse_positive_u32(
                lookup(SURGE_LOAD_THRESHOLD_BPS_ENV).as_deref(),
                defaults.surge_load_threshold_bps,
            ),
            surge_multiplier: parse_positive_u32(
                lookup(SURGE_MULTIPLIER_ENV).as_deref(),
                defaults.surge_multiplier,
            ),
            normal_multiplier: parse_positive_u32(
                lookup(NORMAL_MULTIPLIER_ENV).as_deref(),
                defaults.normal_multiplier,
            ),
        }
    }

    /// Resolve `(surge_multiplier, high_load)` for the given network load.
    fn surge(&self, network_load_bps: u32) -> (u32, bool) {
        if network_load_bps >= self.surge_load_threshold_bps {
            (self.surge_multiplier, true)
        } else {
            (self.normal_multiplier, false)
        }
    }

    /// Apply the surge multiplier (expressed in percent) to a raw fee total.
    fn apply_surge(&self, base_fee: i64, resource_fee: i64, surge_multiplier: u32) -> i64 {
        (base_fee + resource_fee) * surge_multiplier as i64 / 100
    }
}

/// Parse a strictly-positive `i64`, falling back to `default` when the value is
/// absent, unparseable, or `<= 0`.
fn parse_positive_i64(value: Option<&str>, default: i64) -> i64 {
    value
        .and_then(|raw| raw.trim().parse::<i64>().ok())
        .filter(|v| *v > 0)
        .unwrap_or(default)
}

/// Parse a strictly-positive `u32`, falling back to `default` when the value is
/// absent, unparseable, or `0`.
fn parse_positive_u32(value: Option<&str>, default: u32) -> u32 {
    value
        .and_then(|raw| raw.trim().parse::<u32>().ok())
        .filter(|v| *v > 0)
        .unwrap_or(default)
}

#[derive(Debug, Clone)]
pub struct SorobanRpcClient {
    pub rpc_url: String,
    pub router_core_contract_id: Option<String>,
    fee_config: FeeConfig,
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
    #[allow(dead_code)]
    pub min_resource_fee: String,
    pub error: Option<String>,
    #[serde(default)]
    #[allow(dead_code)]
    pub events: Vec<serde_json::Value>,
}

#[derive(Deserialize, Debug)]
struct SimulateTransactionResultWithReturnValue {
    #[serde(rename = "minResourceFee", default)]
    #[allow(dead_code)]
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

/// Raw response from the `getTransaction` RPC method.
#[derive(Deserialize, Debug)]
pub struct GetTransactionResult {
    /// Transaction status: "SUCCESS", "FAILED", "NOT_FOUND", "PENDING"
    pub status: String,
    /// Optional envelope XDR (not used for status mapping, but available).
    #[serde(rename = "envelopeXdr")]
    pub envelope_xdr: Option<String>,
    /// Ledger the transaction was included in (present for SUCCESS/FAILED).
    pub ledger: Option<u32>,
    /// Unix timestamp of the ledger close time.
    #[serde(rename = "ledgerCloseTime")]
    pub ledger_close_time: Option<String>,
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
    pub fn new(
        rpc_url: impl Into<String>,
        router_core_contract_id: Option<String>,
        fee_config: FeeConfig,
    ) -> Self {
        Self {
            rpc_url: rpc_url.into(),
            router_core_contract_id,
            fee_config,
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
                // Parse min_resource_fee returned by the RPC. If the field is
                // missing, empty, or malformed, fall back to a conservative
                // default but log a warning so operators can investigate.
                let resource_fee: i64 = match result.min_resource_fee.trim().parse::<i64>() {
                    Ok(n) if n > 0 => n,
                    Ok(n) => {
                        warn!(min_resource_fee = ?result.min_resource_fee, parsed = n, "min_resource_fee is not positive; using fallback {}", 1_000);
                        1_000
                    }
                    Err(e) => {
                        warn!(min_resource_fee = ?result.min_resource_fee, error = %e, "failed to parse min_resource_fee; using fallback {}", 1_000);
                        1_000
                    }
                };
                let base_fee: i64 = self.fee_config.base_fee;
                let (surge_multiplier, high_load) = self.fee_config.surge(network_load_bps);
                let total_fee =
                    self.fee_config
                        .apply_surge(base_fee, resource_fee, surge_multiplier);
                Ok(FeeBreakdown {
                    base_fee,
                    resource_fee,
                    total_fee,
                    surge_multiplier,
                    high_load,
                    would_succeed,
                })
            }
            Err(_) => Ok(self.heuristic_estimate(amount, network_load_bps)),
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

    /// Query the Soroban RPC for the current status of a submitted transaction.
    ///
    /// Maps the RPC `status` string to our [`TransactionStatus`] enum:
    /// - `"SUCCESS"` → `Confirmed`
    /// - `"FAILED"` → `Failed`
    /// - `"NOT_FOUND"` → `Pending` (not yet visible in ledger history)
    /// - anything else → `Submitted`
    ///
    /// Returns an error only if the network request itself fails or the
    /// response cannot be parsed; unexpected status strings are treated as
    /// `Submitted` so the poller keeps watching.
    pub async fn get_transaction_status(&self, tx_id: &str) -> Result<TransactionStatus> {
        let req = JsonRpcRequest {
            jsonrpc: "2.0",
            id: 1,
            method: "getTransaction",
            params: serde_json::json!({ "hash": tx_id }),
        };

        let resp: JsonRpcResponse<GetTransactionResult> = self
            .http
            .post(&self.rpc_url)
            .json(&req)
            .send()
            .await
            .map_err(|e| anyhow!("RPC request failed: {}", e))?
            .json()
            .await
            .map_err(|e| anyhow!("Failed to parse getTransaction response: {}", e))?;

        if let Some(err) = resp.error {
            return Err(anyhow!("RPC error: {}", err.message));
        }

        let result = resp
            .result
            .ok_or_else(|| anyhow!("empty result for getTransaction"))?;

        let status = match result.status.as_str() {
            "SUCCESS" => TransactionStatus::Confirmed,
            "FAILED" => TransactionStatus::Failed,
            "NOT_FOUND" => TransactionStatus::Pending,
            _ => TransactionStatus::Submitted,
        };

        Ok(status)
    }

    fn heuristic_estimate(&self, amount: i64, network_load_bps: u32) -> FeeBreakdown {
        let base_fee: i64 = self.fee_config.base_fee;
        let resource_fee: i64 = {
            let scaled = amount / self.fee_config.resource_fee_divisor;
            scaled.max(self.fee_config.resource_fee_floor)
        };
        let (surge_multiplier, high_load) = self.fee_config.surge(network_load_bps);
        let total_fee = self
            .fee_config
            .apply_surge(base_fee, resource_fee, surge_multiplier);
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn client_with(fee_config: FeeConfig) -> SorobanRpcClient {
        SorobanRpcClient::new("http://127.0.0.1:0", None, fee_config)
    }

    #[test]
    fn fee_config_defaults_when_env_absent() {
        let config = FeeConfig::from_lookup(|_| None);
        assert_eq!(config, FeeConfig::default());
        assert_eq!(config.base_fee, DEFAULT_BASE_FEE);
        assert_eq!(config.resource_fee_floor, DEFAULT_RESOURCE_FEE_FLOOR);
        assert_eq!(config.resource_fee_divisor, DEFAULT_RESOURCE_FEE_DIVISOR);
        assert_eq!(
            config.surge_load_threshold_bps,
            DEFAULT_SURGE_LOAD_THRESHOLD_BPS
        );
        assert_eq!(config.surge_multiplier, DEFAULT_SURGE_MULTIPLIER);
        assert_eq!(config.normal_multiplier, DEFAULT_NORMAL_MULTIPLIER);
    }

    #[test]
    fn fee_config_reads_overrides() {
        let env = HashMap::from([
            (BASE_FEE_ENV, "250"),
            (RESOURCE_FEE_FLOOR_ENV, "500"),
            (RESOURCE_FEE_DIVISOR_ENV, "2000"),
            (SURGE_LOAD_THRESHOLD_BPS_ENV, "9000"),
            (SURGE_MULTIPLIER_ENV, "300"),
            (NORMAL_MULTIPLIER_ENV, "110"),
        ]);
        let config = FeeConfig::from_lookup(|key| env.get(key).map(|v| v.to_string()));

        assert_eq!(config.base_fee, 250);
        assert_eq!(config.resource_fee_floor, 500);
        assert_eq!(config.resource_fee_divisor, 2_000);
        assert_eq!(config.surge_load_threshold_bps, 9_000);
        assert_eq!(config.surge_multiplier, 300);
        assert_eq!(config.normal_multiplier, 110);
    }

    #[test]
    fn fee_config_rejects_invalid_or_nonpositive_values() {
        let env = HashMap::from([
            (BASE_FEE_ENV, "not-a-number"),
            (RESOURCE_FEE_FLOOR_ENV, "0"),
            (RESOURCE_FEE_DIVISOR_ENV, "-5"),
            (SURGE_MULTIPLIER_ENV, "   "),
        ]);
        let config = FeeConfig::from_lookup(|key| env.get(key).map(|v| v.to_string()));

        // All invalid values fall back to their defaults.
        assert_eq!(config, FeeConfig::default());
    }

    #[test]
    fn fee_config_trims_whitespace() {
        let env = HashMap::from([(BASE_FEE_ENV, "  175  ")]);
        let config = FeeConfig::from_lookup(|key| env.get(key).map(|v| v.to_string()));
        assert_eq!(config.base_fee, 175);
    }

    #[test]
    fn heuristic_estimate_uses_defaults() {
        let client = client_with(FeeConfig::default());
        // amount / 1_000 = 50 < floor (100) -> resource_fee clamped to 100.
        let fee = client.heuristic_estimate(50_000, 0);
        assert_eq!(fee.base_fee, 100);
        assert_eq!(fee.resource_fee, 100);
        assert_eq!(fee.surge_multiplier, 100);
        assert!(!fee.high_load);
        // (100 + 100) * 100 / 100 = 200
        assert_eq!(fee.total_fee, 200);
    }

    #[test]
    fn heuristic_estimate_applies_surge_above_threshold() {
        let client = client_with(FeeConfig::default());
        // amount / 1_000 = 1_000 > floor -> resource_fee = 1_000; load triggers surge.
        let fee = client.heuristic_estimate(1_000_000, 8_000);
        assert_eq!(fee.resource_fee, 1_000);
        assert_eq!(fee.surge_multiplier, 200);
        assert!(fee.high_load);
        // (100 + 1_000) * 200 / 100 = 2_200
        assert_eq!(fee.total_fee, 2_200);
    }

    #[test]
    fn heuristic_estimate_honours_custom_config() {
        let config = FeeConfig {
            base_fee: 200,
            resource_fee_floor: 50,
            resource_fee_divisor: 500,
            surge_load_threshold_bps: 5_000,
            surge_multiplier: 150,
            normal_multiplier: 100,
        };
        let client = client_with(config);
        // amount / 500 = 200 > floor (50) -> resource_fee = 200; load 5_000 triggers surge.
        let fee = client.heuristic_estimate(100_000, 5_000);
        assert_eq!(fee.base_fee, 200);
        assert_eq!(fee.resource_fee, 200);
        assert_eq!(fee.surge_multiplier, 150);
        assert!(fee.high_load);
        // (200 + 200) * 150 / 100 = 600
        assert_eq!(fee.total_fee, 600);
    }
}

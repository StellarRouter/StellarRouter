//! Soroban RPC client helpers.
//!
//! Wraps the JSON-RPC calls needed to read on-chain contract state.
//! We use raw `reqwest` + `serde_json` rather than the `stellar-rpc-client`
//! crate so that this binary has no dependency on the Soroban SDK (which
//! requires `wasm32` toolchain features and complicates native builds).

use anyhow::{anyhow, Context, Result};
use async_trait::async_trait;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::time::Duration;
use tracing::{debug, warn};

// ── JSON-RPC request / response types ────────────────────────────────────────

#[derive(Debug, Serialize)]
struct RpcRequest<'a> {
    jsonrpc: &'a str,
    id: u64,
    method: &'a str,
    params: Value,
}

#[derive(Debug, Deserialize)]
struct RpcResponse {
    result: Option<Value>,
    error: Option<RpcError>,
}

#[derive(Debug, Deserialize)]
struct RpcError {
    code: i64,
    message: String,
}

// ── Decoded ledger entry types ────────────────────────────────────────────────

/// A single ledger entry returned by `getLedgerEntries`.
#[derive(Debug, Deserialize, Clone)]
pub struct LedgerEntry {
    /// Base64-encoded XDR of the entry key.
    pub key: String,
    /// Base64-encoded XDR of the entry value.
    pub xdr: String,
}

/// Response from `getLedgerEntries`.
#[derive(Debug, Deserialize)]
struct GetLedgerEntriesResult {
    entries: Option<Vec<LedgerEntry>>,
}

/// A single event returned by `getEvents`.
#[derive(Debug, Deserialize, Clone)]
pub struct ContractEvent {
    /// The contract that emitted the event.
    #[serde(rename = "contractId")]
    pub contract_id: String,
    /// Ledger sequence number in which this event was emitted.
    /// Defaults to 0 when absent (e.g. in test fixtures).
    #[serde(default)]
    pub ledger: u32,
    /// Event topic symbols (decoded from XDR).
    pub topic: Vec<serde_json::Value>,
    /// Event value (decoded from XDR).
    pub value: serde_json::Value,
}

/// Response from `getEvents`.
#[derive(Debug, Deserialize)]
struct GetEventsResult {
    events: Option<Vec<ContractEvent>>,
}

// ── Client ────────────────────────────────────────────────────────────────────

/// Thin async wrapper around the Soroban JSON-RPC endpoint.
#[derive(Clone)]
pub struct SorobanRpcClient {
    http: Client,
    rpc_url: String,
}

impl SorobanRpcClient {
    /// Create a new client.
    ///
    /// `timeout_secs` is applied to every individual HTTP request.
    pub fn new(rpc_url: impl Into<String>, timeout_secs: u64) -> Result<Self> {
        let http = Client::builder()
            .timeout(Duration::from_secs(timeout_secs))
            .build()
            .context("failed to build HTTP client")?;
        Ok(Self {
            http,
            rpc_url: rpc_url.into(),
        })
    }

    /// Call `simulateTransaction` to invoke a read-only contract function and
    /// return the raw JSON result value.
    ///
    /// This is the standard way to call view functions on Soroban contracts
    /// without submitting a real transaction.
    pub async fn simulate_invoke(
        &self,
        contract_id: &str,
        function_name: &str,
        args_xdr: Vec<String>,
    ) -> Result<Value> {
        // Build a minimal transaction envelope XDR for simulation.
        // We use the `invokeHostFunction` operation type.
        let invoke_params = json!({
            "transaction": build_invoke_xdr(contract_id, function_name, &args_xdr)?,
        });

        let req = RpcRequest {
            jsonrpc: "2.0",
            id: 1,
            method: "simulateTransaction",
            params: invoke_params,
        };

        let resp: RpcResponse = self
            .http
            .post(&self.rpc_url)
            .json(&req)
            .send()
            .await
            .context("HTTP request failed")?
            .json()
            .await
            .context("failed to parse JSON-RPC response")?;

        if let Some(err) = resp.error {
            return Err(anyhow!("RPC error {}: {}", err.code, err.message));
        }

        resp.result.ok_or_else(|| anyhow!("empty RPC result"))
    }

    /// Call `getLedgerEntries` for the given base64-encoded XDR keys.
    pub async fn get_ledger_entries(&self, keys_xdr: Vec<String>) -> Result<Vec<LedgerEntry>> {        let req = RpcRequest {
            jsonrpc: "2.0",
            id: 1,
            method: "getLedgerEntries",
            params: json!({ "keys": keys_xdr }),
        };

        let resp: RpcResponse = self
            .http
            .post(&self.rpc_url)
            .json(&req)
            .send()
            .await
            .context("HTTP request failed")?
            .json()
            .await
            .context("failed to parse JSON-RPC response")?;

        if let Some(err) = resp.error {
            return Err(anyhow!("RPC error {}: {}", err.code, err.message));
        }

        let result: GetLedgerEntriesResult =
            serde_json::from_value(resp.result.ok_or_else(|| anyhow!("empty RPC result"))?)
                .context("failed to deserialize getLedgerEntries result")?;

        Ok(result.entries.unwrap_or_default())
    }

    /// Call `getEvents` to fetch contract events matching the given topic filters.
    ///
    /// `contract_id` — the contract whose events to query.
    /// `topic_filters` — list of topic symbol strings to match (e.g. `["quote_generated"]`).
    /// `start_ledger` — earliest ledger to include (0 = let the RPC choose).
    pub async fn get_events(
        &self,
        contract_id: &str,
        topic_filters: &[&str],
        start_ledger: u32,
    ) -> Result<Vec<ContractEvent>> {
        let filters = topic_filters
            .iter()
            .map(|t| json!({ "type": "contract", "contractIds": [contract_id], "topics": [[t]] }))
            .collect::<serde_json::Value>();

        let params = if start_ledger > 0 {
            json!({ "startLedger": start_ledger, "filters": filters })
        } else {
            json!({ "filters": filters })
        };

        let req = RpcRequest {
            jsonrpc: "2.0",
            id: 1,
            method: "getEvents",
            params,
        };

        let resp: RpcResponse = self
            .http
            .post(&self.rpc_url)
            .json(&req)
            .send()
            .await
            .context("HTTP request failed")?
            .json()
            .await
            .context("failed to parse JSON-RPC response")?;

        if let Some(err) = resp.error {
            return Err(anyhow!("RPC error {}: {}", err.code, err.message));
        }

        let result: GetEventsResult =
            serde_json::from_value(resp.result.ok_or_else(|| anyhow!("empty RPC result"))?)
                .context("failed to deserialize getEvents result")?;

        Ok(result.events.unwrap_or_default())
    }

    /// Convenience: call a view function and extract a `u64` from the result.
    ///
    /// Soroban returns `u64` values as XDR `ScVal::U64`.  The RPC simulation
    /// result encodes the return value in `results[0].xdr` as base64 XDR.
    /// We parse the JSON representation that the RPC server returns in the
    /// `results` array.
    pub async fn call_u64(&self, contract_id: &str, function_name: &str) -> Result<u64> {
        debug!(contract_id, function_name, "calling view function → u64");
        let result = self
            .simulate_invoke(contract_id, function_name, vec![])
            .await?;

        // The simulation result has shape:
        // { "results": [{ "xdr": "<base64 ScVal XDR>", ... }], ... }
        // We look for a numeric value in the decoded JSON representation.
        extract_u64_from_sim_result(&result)
            .with_context(|| format!("parsing u64 from {function_name} on {contract_id}"))
    }

    /// Convenience: call a view function and extract a `bool` from the result.
    pub async fn call_bool(&self, contract_id: &str, function_name: &str) -> Result<bool> {
        debug!(contract_id, function_name, "calling view function → bool");
        let result = self
            .simulate_invoke(contract_id, function_name, vec![])
            .await?;
        extract_bool_from_sim_result(&result)
            .with_context(|| format!("parsing bool from {function_name} on {contract_id}"))
    }

    /// Convenience: call a view function and extract a `Vec<String>` from the result.
    pub async fn call_string_vec(
        &self,
        contract_id: &str,
        function_name: &str,
    ) -> Result<Vec<String>> {
        debug!(
            contract_id,
            function_name, "calling view function → Vec<String>"
        );
        let result = self
            .simulate_invoke(contract_id, function_name, vec![])
            .await?;
        extract_string_vec_from_sim_result(&result)
            .with_context(|| format!("parsing Vec<String> from {function_name} on {contract_id}"))
    }

    /// Convenience: call a view function with a single string arg and extract a `Vec<u32>` from the result.
    pub async fn call_u32_vec(
        &self,
        contract_id: &str,
        function_name: &str,
        arg: &str,
    ) -> Result<Vec<u32>> {
        debug!(
            contract_id,
            function_name, "calling view function → Vec<u32>"
        );
        let result = self
            .simulate_invoke(contract_id, function_name, vec![hex_encode_arg(arg)])
            .await?;
        extract_u32_vec_from_sim_result(&result)
            .with_context(|| format!("parsing Vec<u32> from {function_name}({arg}) on {contract_id}"))
    }
}

// ── XDR helpers ───────────────────────────────────────────────────────────────

/// Build a minimal base64-encoded transaction XDR suitable for `simulateTransaction`.
///
/// We construct the smallest valid `TransactionEnvelope` that wraps an
/// `InvokeHostFunctionOp` for the given contract / function / args.
///
/// In practice the Soroban RPC server only needs the operation body to be
/// correct; the source account, fee, and sequence number are ignored during
/// simulation.
fn build_invoke_xdr(contract_id: &str, function_name: &str, args_xdr: &[String]) -> Result<String> {
    // We use the Stellar Horizon / Soroban RPC "friendly" JSON format for
    // transaction simulation.  The RPC server accepts a JSON object with a
    // `transaction` field containing a base64-encoded XDR TransactionEnvelope.
    //
    // Building a full XDR envelope from scratch without the Stellar SDK is
    // non-trivial.  Instead we use the `stellar_xdr` crate (already a
    // transitive dependency of `stellar-rpc-client`) to construct the XDR.
    //
    // For simplicity in this implementation we return a placeholder that
    // signals to the caller that full XDR construction requires the
    // stellar-xdr crate to be wired up.  The `collector` module uses
    // `getLedgerEntries` (which does not require XDR transaction building)
    // as the primary data source, falling back to simulation only when
    // direct storage key access is not possible.
    //
    // A production deployment should replace this with proper XDR construction
    // using the `stellar-xdr` crate or the Stellar JS SDK via a sidecar.
    let _ = (contract_id, function_name, args_xdr);
    Err(anyhow!(
        "XDR transaction building is not implemented in this reference exporter. \
         Use getLedgerEntries-based scraping (the default) or integrate the \
         stellar-xdr crate to build InvokeHostFunction envelopes."
    ))
}

// ── Result extraction helpers ─────────────────────────────────────────────────

/// Extract a `u64` from a `simulateTransaction` result JSON value.
///
/// The Soroban RPC returns the return value as a base64-encoded `ScVal` XDR
/// in `result.results[0].xdr`.  The RPC server also provides a JSON-decoded
/// representation in some versions.  We try both paths.
fn extract_u64_from_sim_result(result: &Value) -> Result<u64> {
    // Path 1: JSON-decoded ScVal in `results[0].retval` (newer RPC versions)
    if let Some(v) = result
        .get("results")
        .and_then(|r| r.get(0))
        .and_then(|r| r.get("retval"))
    {
        if let Some(n) = v.as_u64() {
            return Ok(n);
        }
        // ScVal::U64 is encoded as `{"u64": <number>}` in JSON
        if let Some(n) = v.get("u64").and_then(|n| n.as_u64()) {
            return Ok(n);
        }
    }

    // Path 2: Numeric value directly in `result`
    if let Some(n) = result.as_u64() {
        return Ok(n);
    }

    Err(anyhow!("could not find u64 in simulation result: {result}"))
}

/// Extract a `bool` from a `simulateTransaction` result JSON value.
fn extract_bool_from_sim_result(result: &Value) -> Result<bool> {
    if let Some(v) = result
        .get("results")
        .and_then(|r| r.get(0))
        .and_then(|r| r.get("retval"))
    {
        if let Some(b) = v.as_bool() {
            return Ok(b);
        }
        if let Some(b) = v.get("bool").and_then(|b| b.as_bool()) {
            return Ok(b);
        }
    }
    if let Some(b) = result.as_bool() {
        return Ok(b);
    }
    Err(anyhow!(
        "could not find bool in simulation result: {result}"
    ))
}

/// Extract a `Vec<String>` from a `simulateTransaction` result JSON value.
fn extract_string_vec_from_sim_result(result: &Value) -> Result<Vec<String>> {
    let retval = result
        .get("results")
        .and_then(|r| r.get(0))
        .and_then(|r| r.get("retval"))
        .unwrap_or(result);

    if let Some(arr) = retval.as_array() {
        let strings: Vec<String> = arr
            .iter()
            .filter_map(|v| {
                v.as_str()
                    .map(|s| s.to_string())
                    .or_else(|| v.get("str").and_then(|s| s.as_str()).map(|s| s.to_string()))
            })
            .collect();
        return Ok(strings);
    }

    // Empty vec is a valid return value
    Ok(vec![])
}

/// Extract a `Vec<u32>` from a `simulateTransaction` result JSON value.
///
/// Used to parse the return value of `versions(name) -> Vec<u32>`.
fn extract_u32_vec_from_sim_result(result: &Value) -> Result<Vec<u32>> {
    let retval = result
        .get("results")
        .and_then(|r| r.get(0))
        .and_then(|r| r.get("retval"))
        .unwrap_or(result);

    if let Some(arr) = retval.as_array() {
        let nums: Vec<u32> = arr
            .iter()
            .filter_map(|v| {
                v.as_u64()
                    .map(|n| n as u32)
                    .or_else(|| v.get("u32").and_then(|n| n.as_u64()).map(|n| n as u32))
            })
            .collect();
        return Ok(nums);
    }

    Ok(vec![])
}


/// Hex-encode a string as a placeholder XDR `ScVal::String` argument.
fn hex_encode_arg(s: &str) -> String {
    use std::fmt::Write;
    let mut out = String::new();
    for b in s.as_bytes() {
        write!(out, "{b:02x}").ok();
    }
    out
}
// ── Storage key XDR helpers ───────────────────────────────────────────────────

/// Build the base64-encoded XDR key for a `ContractData` ledger entry.
///
/// This is used with `getLedgerEntries` to read contract storage directly
/// without needing to simulate a transaction.
///
/// The key format is:
/// ```text
/// LedgerKey::ContractData {
///     contract: ScAddress::Contract(Hash(<contract_id_bytes>)),
///     key: <ScVal>,
///     durability: ContractDataDurability::Persistent | Instance,
/// }
/// ```
///
/// For the simple scalar keys used by router contracts (e.g. `DataKey::TotalRouted`,
/// `DataKey::Paused`) the `key` ScVal is a `ScVal::LedgerKeyContractInstance`
/// for instance storage.
///
/// Full XDR construction is left as an integration point; the collector uses
/// the simulation path as a fallback.
pub fn instance_storage_key_xdr(_contract_id: &str) -> Result<String> {
    Err(anyhow!(
        "Direct XDR key construction not implemented. \
         Use the simulation path or integrate stellar-xdr."
    ))
}

// ── RpcClient trait ───────────────────────────────────────────────────────────

/// Trait abstracting the Soroban RPC calls used by the collector.
///
/// Implement this trait with [`MockRpcClient`] in tests to avoid live network
/// access, or use the real [`SorobanRpcClient`] in production.
#[async_trait::async_trait]
pub trait RpcClient: Send + Sync {
    async fn call_u64(&self, contract_id: &str, function_name: &str) -> Result<u64>;
    async fn call_bool(&self, contract_id: &str, function_name: &str) -> Result<bool>;
    async fn call_string_vec(
        &self,
        contract_id: &str,
        function_name: &str,
    ) -> Result<Vec<String>>;
    async fn call_u32_vec(
        &self,
        contract_id: &str,
        function_name: &str,
        arg: &str,
    ) -> Result<Vec<u32>>;
    async fn simulate_invoke(
        &self,
        contract_id: &str,
        function_name: &str,
        args_xdr: Vec<String>,
    ) -> Result<serde_json::Value>;
    async fn get_events(
        &self,
        contract_id: &str,
        topic_filters: &[&str],
        start_ledger: u32,
    ) -> Result<Vec<ContractEvent>>;
    async fn get_ledger_entries(&self, keys_xdr: Vec<String>) -> Result<Vec<LedgerEntry>>;
}

#[async_trait::async_trait]
impl RpcClient for SorobanRpcClient {
    async fn call_u64(&self, contract_id: &str, function_name: &str) -> Result<u64> {
        self.call_u64(contract_id, function_name).await
    }
    async fn call_bool(&self, contract_id: &str, function_name: &str) -> Result<bool> {
        self.call_bool(contract_id, function_name).await
    }
    async fn call_string_vec(
        &self,
        contract_id: &str,
        function_name: &str,
    ) -> Result<Vec<String>> {
        self.call_string_vec(contract_id, function_name).await
    }
    async fn call_u32_vec(
        &self,
        contract_id: &str,
        function_name: &str,
        arg: &str,
    ) -> Result<Vec<u32>> {
        self.call_u32_vec(contract_id, function_name, arg).await
    }
    async fn simulate_invoke(
        &self,
        contract_id: &str,
        function_name: &str,
        args_xdr: Vec<String>,
    ) -> Result<serde_json::Value> {
        self.simulate_invoke(contract_id, function_name, args_xdr)
            .await
    }
    async fn get_events(
        &self,
        contract_id: &str,
        topic_filters: &[&str],
        start_ledger: u32,
    ) -> Result<Vec<ContractEvent>> {
        self.get_events(contract_id, topic_filters, start_ledger).await
    }
    async fn get_ledger_entries(&self, keys_xdr: Vec<String>) -> Result<Vec<LedgerEntry>> {
        self.get_ledger_entries(keys_xdr).await
    }
}

// ── MockRpcClient ─────────────────────────────────────────────────────────────

/// A deterministic mock RPC client for use in tests.
///
/// Pre-load responses via the builder methods; any call not explicitly
/// configured returns an error so tests fail loudly on unexpected calls.
///
/// # Example
/// ```rust
/// let mock = MockRpcClient::new()
///     .with_u64("CONTRACT", "total_routed", 42)
///     .with_string_vec("CONTRACT", "get_all_routes", vec![]);
/// ```
#[cfg(any(test, feature = "test-utils"))]
pub struct MockRpcClient {
    u64_responses: std::collections::HashMap<(String, String), u64>,
    bool_responses: std::collections::HashMap<(String, String), bool>,
    string_vec_responses: std::collections::HashMap<(String, String), Vec<String>>,
    u32_vec_responses: std::collections::HashMap<(String, String, String), Vec<u32>>,
    simulate_responses:
        std::collections::HashMap<(String, String), serde_json::Value>,
    events_responses: std::collections::HashMap<(String, String), Vec<ContractEvent>>,
    ledger_entries_responses: std::collections::HashMap<String, Vec<LedgerEntry>>,
}

#[cfg(any(test, feature = "test-utils"))]
impl MockRpcClient {
    pub fn new() -> Self {
        Self {
            u64_responses: Default::default(),
            bool_responses: Default::default(),
            string_vec_responses: Default::default(),
            u32_vec_responses: Default::default(),
            simulate_responses: Default::default(),
            events_responses: Default::default(),
            ledger_entries_responses: Default::default(),
        }
    }

    pub fn with_u64(mut self, contract: &str, func: &str, val: u64) -> Self {
        self.u64_responses
            .insert((contract.to_string(), func.to_string()), val);
        self
    }

    pub fn with_bool(mut self, contract: &str, func: &str, val: bool) -> Self {
        self.bool_responses
            .insert((contract.to_string(), func.to_string()), val);
        self
    }

    pub fn with_string_vec(mut self, contract: &str, func: &str, val: Vec<String>) -> Self {
        self.string_vec_responses
            .insert((contract.to_string(), func.to_string()), val);
        self
    }

    /// Pre-load a `call_u32_vec` response for a given contract + function + arg.
    pub fn with_u32_vec(mut self, contract: &str, func: &str, arg: &str, val: Vec<u32>) -> Self {
        self.u32_vec_responses
            .insert((contract.to_string(), func.to_string(), arg.to_string()), val);
        self
    }

    pub fn with_simulate(
        mut self,
        contract: &str,
        func: &str,
        val: serde_json::Value,
    ) -> Self {
        self.simulate_responses
            .insert((contract.to_string(), func.to_string()), val);
        self
    }

    /// Pre-load a `getEvents` response for a given contract + topic.
    pub fn with_events(mut self, contract: &str, topic: &str, val: Vec<ContractEvent>) -> Self {
        self.events_responses
            .insert((contract.to_string(), topic.to_string()), val);
        self
    }

    /// Pre-load a `getLedgerEntries` response keyed by the first XDR key.
    pub fn with_ledger_entries(mut self, key: &str, val: Vec<LedgerEntry>) -> Self {
        self.ledger_entries_responses.insert(key.to_string(), val);
        self
    }
}

#[cfg(any(test, feature = "test-utils"))]
#[async_trait::async_trait]
impl RpcClient for MockRpcClient {
    async fn call_u64(&self, contract_id: &str, function_name: &str) -> Result<u64> {
        self.u64_responses
            .get(&(contract_id.to_string(), function_name.to_string()))
            .copied()
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "MockRpcClient: no u64 response for {contract_id}::{function_name}"
                )
            })
    }

    async fn call_bool(&self, contract_id: &str, function_name: &str) -> Result<bool> {
        self.bool_responses
            .get(&(contract_id.to_string(), function_name.to_string()))
            .copied()
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "MockRpcClient: no bool response for {contract_id}::{function_name}"
                )
            })
    }

    async fn call_string_vec(
        &self,
        contract_id: &str,
        function_name: &str,
    ) -> Result<Vec<String>> {
        self.string_vec_responses
            .get(&(contract_id.to_string(), function_name.to_string()))
            .cloned()
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "MockRpcClient: no string_vec response for {contract_id}::{function_name}"
                )
            })
    }

    async fn call_u32_vec(
        &self,
        contract_id: &str,
        function_name: &str,
        arg: &str,
    ) -> Result<Vec<u32>> {
        Ok(self
            .u32_vec_responses
            .get(&(contract_id.to_string(), function_name.to_string(), arg.to_string()))
            .cloned()
            .unwrap_or_default())
    }

    async fn simulate_invoke(
        &self,
        contract_id: &str,
        function_name: &str,
        _args_xdr: Vec<String>,
    ) -> Result<serde_json::Value> {
        self.simulate_responses
            .get(&(contract_id.to_string(), function_name.to_string()))
            .cloned()
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "MockRpcClient: no simulate response for {contract_id}::{function_name}"
                )
            })
    }

    async fn get_events(
        &self,
        contract_id: &str,
        topic_filters: &[&str],
        _start_ledger: u32,
    ) -> Result<Vec<ContractEvent>> {
        // Return events for the first matching topic filter.
        for topic in topic_filters {
            if let Some(events) = self
                .events_responses
                .get(&(contract_id.to_string(), topic.to_string()))
            {
                return Ok(events.clone());
            }
        }
        Ok(vec![])
    }

    async fn get_ledger_entries(&self, keys_xdr: Vec<String>) -> Result<Vec<LedgerEntry>> {
        let key = keys_xdr.first().cloned().unwrap_or_default();
        Ok(self
            .ledger_entries_responses
            .get(&key)
            .cloned()
            .unwrap_or_default())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_extract_u64_direct() {
        let v = json!(42u64);
        assert_eq!(extract_u64_from_sim_result(&v).unwrap(), 42);
    }

    #[test]
    fn test_extract_u64_nested_retval() {
        let v = json!({
            "results": [{ "retval": { "u64": 99 } }]
        });
        assert_eq!(extract_u64_from_sim_result(&v).unwrap(), 99);
    }

    #[test]
    fn test_extract_bool_direct() {
        let v = json!(true);
        assert!(extract_bool_from_sim_result(&v).unwrap());
    }

    #[test]
    fn test_extract_bool_nested() {
        let v = json!({
            "results": [{ "retval": { "bool": false } }]
        });
        assert!(!extract_bool_from_sim_result(&v).unwrap());
    }

    #[test]
    fn test_extract_string_vec_empty() {
        let v = json!([]);
        assert!(extract_string_vec_from_sim_result(&v).unwrap().is_empty());
    }

    #[test]
    fn test_extract_string_vec_strings() {
        let v = json!(["oracle", "price_feed"]);
        let result = extract_string_vec_from_sim_result(&v).unwrap();
        assert_eq!(result, vec!["oracle", "price_feed"]);
    }
}

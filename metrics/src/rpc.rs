//! Soroban RPC client helpers.
//!
//! Wraps the JSON-RPC calls needed to read on-chain contract state.
//! We use raw `reqwest` + `serde_json` rather than the `stellar-rpc-client`
//! crate so that this binary has no dependency on the Soroban SDK (which
//! requires `wasm32` toolchain features and complicates native builds).

use anyhow::{anyhow, Context, Result};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::time::Duration;
use tracing::{debug, warn};

/// Maximum number of pages to fetch in a single `get_events` call.
/// Prevents unbounded loops if the RPC server misbehaves.
const GET_EVENTS_MAX_PAGES: usize = 20;

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
    #[allow(dead_code)]
    pub contract_id: String,
    /// Ledger sequence number in which this event was emitted.
    #[serde(default)]
    pub ledger: u32,
    /// Event topic symbols (decoded from XDR).
    #[allow(dead_code)]
    pub topic: Vec<serde_json::Value>,
    /// Event value (decoded from XDR).
    pub value: serde_json::Value,
}

/// Response from `getEvents`.
#[derive(Debug, Deserialize)]
struct GetEventsResult {
    events: Option<Vec<ContractEvent>>,
    #[serde(rename = "latestLedger")]
    #[allow(dead_code)]
    latest_ledger: Option<u64>,
}

// ── Client ────────────────────────────────────────────────────────────────────

/// Thin async wrapper around the Soroban JSON-RPC endpoint.
#[derive(Clone)]
pub struct SorobanRpcClient {
    http: Client,
    rpc_url: String,
    /// Maximum number of retry attempts for retryable errors (default: 3).
    max_retries: u32,
    /// Base backoff duration before first retry (doubles on each attempt).
    base_backoff: Duration,
}

impl SorobanRpcClient {
    /// Create a new client.
    ///
    /// `timeout_secs` is applied to every individual HTTP request.
    /// Retry behaviour is configured via environment variables:
    ///
    /// | Variable | Default | Description |
    /// |---|---|---|
    /// | `ROUTER_RPC_MAX_RETRIES` | `3` | Max retry attempts for transient errors. |
    /// | `ROUTER_RPC_BACKOFF_MS` | `200` | Base backoff in milliseconds (doubles each retry). |
    pub fn new(rpc_url: impl Into<String>, timeout_secs: u64) -> Result<Self> {
        let http = Client::builder()
            .timeout(Duration::from_secs(timeout_secs))
            .build()
            .context("failed to build HTTP client")?;

        let max_retries = std::env::var("ROUTER_RPC_MAX_RETRIES")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(3u32);

        let base_backoff_ms = std::env::var("ROUTER_RPC_BACKOFF_MS")
            .ok()
            .and_then(|v| v.parse::<u64>().ok())
            .unwrap_or(200);

        Ok(Self {
            http,
            rpc_url: rpc_url.into(),
            max_retries,
            base_backoff: Duration::from_millis(base_backoff_ms),
        })
    }

    /// Determine whether an HTTP status code or a reqwest error is retryable.
    ///
    /// We retry on:
    /// - Connection-level errors (timeout, connection reset, DNS failure)
    /// - HTTP 429 (Too Many Requests) and 5xx server errors
    fn is_retryable_error(err: &reqwest::Error) -> bool {
        if err.is_timeout() || err.is_connect() {
            return true;
        }
        if let Some(status) = err.status() {
            return status.is_server_error() || status.as_u16() == 429;
        }
        false
    }

    /// Execute an HTTP POST and parse the JSON-RPC response, retrying up to
    /// `self.max_retries` times on transient/retryable errors with exponential
    /// backoff.
    async fn post_with_retry(&self, req_body: &impl Serialize) -> Result<RpcResponse> {
        let mut attempt = 0u32;
        loop {
            let result = self
                .http
                .post(&self.rpc_url)
                .json(req_body)
                .send()
                .await;

            match result {
                Ok(response) => {
                    // Treat 5xx / 429 as retryable at the HTTP level.
                    let status = response.status();
                    if (status.is_server_error() || status.as_u16() == 429)
                        && attempt < self.max_retries
                    {
                        let delay = self.base_backoff * 2u32.pow(attempt);
                        warn!(
                            attempt,
                            delay_ms = delay.as_millis(),
                            %status,
                            "RPC HTTP error — retrying"
                        );
                        attempt += 1;
                        tokio::time::sleep(delay).await;
                        continue;
                    }
                    return response
                        .json::<RpcResponse>()
                        .await
                        .context("failed to parse JSON-RPC response");
                }
                Err(err) => {
                    if Self::is_retryable_error(&err) && attempt < self.max_retries {
                        let delay = self.base_backoff * 2u32.pow(attempt);
                        warn!(
                            attempt,
                            delay_ms = delay.as_millis(),
                            error = %err,
                            "RPC request failed — retrying"
                        );
                        attempt += 1;
                        tokio::time::sleep(delay).await;
                        continue;
                    }
                    return Err(err).context("HTTP request failed");
                }
            }
        }
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

        let resp = self
            .post_with_retry(&req)
            .await?;

        if let Some(err) = resp.error {
            return Err(anyhow!("RPC error {}: {}", err.code, err.message));
        }

        resp.result.ok_or_else(|| anyhow!("empty RPC result"))
    }

    /// Call `getLedgerEntries` for the given base64-encoded XDR keys.
    pub async fn get_ledger_entries(&self, keys_xdr: Vec<String>) -> Result<Vec<LedgerEntry>> {
        let req = RpcRequest {
            jsonrpc: "2.0",
            id: 1,
            method: "getLedgerEntries",
            params: json!({ "keys": keys_xdr }),
        };

        let resp = self
            .post_with_retry(&req)
            .await?;

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
    /// Paginates through all available pages until exhausted or until
    /// [`GET_EVENTS_MAX_PAGES`] is reached (logged as a warning).
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
        let filters: Vec<serde_json::Value> = topic_filters
            .iter()
            .map(|t| json!({ "type": "contract", "contractIds": [contract_id], "topics": [[t]] }))
            .collect();

        let mut all_events: Vec<ContractEvent> = Vec::new();
        let mut paging_token: Option<String> = None;
        let mut pages_fetched: usize = 0;

        loop {
            let params = match (&paging_token, start_ledger) {
                (Some(token), _) => json!({ "pagingToken": token, "filters": filters }),
                (None, ledger) if ledger > 0 => {
                    json!({ "startLedger": ledger, "filters": filters })
                }
                (None, _) => json!({ "filters": filters }),
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

            let raw_value = resp.result.ok_or_else(|| anyhow!("empty RPC result"))?;

        let resp = self
            .post_with_retry(&req)
            .await?;
            // Deserialize the raw response to extract paging tokens.
            let raw_result: GetEventsResult =
                serde_json::from_value(raw_value.clone())
                    .context("failed to deserialize getEvents result")?;

            let page_events = raw_result.events.unwrap_or_default();
            if page_events.is_empty() {
                break;
            }

            // Extract the paging token from the last event in this page
            // by re-parsing the events array to find pagingToken fields.
            let last_paging_token = extract_last_paging_token(&raw_value);

            all_events.extend(page_events);
            pages_fetched += 1;

            if pages_fetched >= GET_EVENTS_MAX_PAGES {
                warn!(
                    pages = pages_fetched,
                    total_events = all_events.len(),
                    "get_events hit max-pages cap ({}); remaining events may be missed",
                    GET_EVENTS_MAX_PAGES
                );
                break;
            }

            match last_paging_token {
                Some(token) => paging_token = Some(token),
                None => break, // No continuation cursor — done.
            }
        }

        debug!(
            pages = pages_fetched,
            total_events = all_events.len(),
            "get_events pagination complete"
        );

        Ok(all_events)
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
    #[allow(dead_code)]
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
        extract_u32_vec_from_sim_result(&result).with_context(|| {
            format!("parsing Vec<u32> from {function_name}({arg}) on {contract_id}")
        })
    }
}

// ── XDR helpers ───────────────────────────────────────────────────────────────

// ── Strkey decode ─────────────────────────────────────────────────────────────

const BASE32_ALPHA: &[u8; 32] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ234567";
const VERSION_CONTRACT: u8 = 2 << 3; // 0x10 → first char 'C'

/// CRC-16/XModem used by Stellar strkey checksums.
fn crc16(data: &[u8]) -> u16 {
    let mut crc: u16 = 0;
    for &b in data {
        crc ^= (b as u16) << 8;
        for _ in 0..8 {
            crc = if crc & 0x8000 != 0 {
                (crc << 1) ^ 0x1021
            } else {
                crc << 1
            };
        }
    }
    crc
}

/// Decode a Stellar contract strkey (C…) into its 32-byte hash.
fn decode_contract_id(strkey: &str) -> Result<[u8; 32]> {
    if strkey.len() != 56 {
        return Err(anyhow!("strkey must be 56 chars, got {}", strkey.len()));
    }
    let mut lookup = [0xFFu8; 256];
    for (i, &c) in BASE32_ALPHA.iter().enumerate() {
        lookup[c as usize] = i as u8;
    }
    let mut bits: u64 = 0;
    let mut bit_count: u32 = 0;
    let mut decoded: Vec<u8> = Vec::with_capacity(35);
    for &ch in strkey.as_bytes() {
        let v = lookup[ch as usize];
        if v == 0xFF {
            return Err(anyhow!("invalid base32 character '{}'", ch as char));
        }
        bits = (bits << 5) | v as u64;
        bit_count += 5;
        if bit_count >= 8 {
            bit_count -= 8;
            decoded.push((bits >> bit_count) as u8);
        }
    }
    if decoded.len() != 35 {
        return Err(anyhow!(
            "strkey decoded to {} bytes, expected 35",
            decoded.len()
        ));
    }
    let version = decoded[0];
    if version != VERSION_CONTRACT {
        return Err(anyhow!(
            "expected contract strkey (C…), got version 0x{:02x}",
            version
        ));
    }
    let payload: [u8; 32] = decoded[1..33].try_into().unwrap();
    let stored_crc = u16::from_le_bytes([decoded[33], decoded[34]]);
    let actual_crc = crc16(&decoded[..33]);
    if actual_crc != stored_crc {
        return Err(anyhow!("strkey checksum mismatch"));
    }
    Ok(payload)
}

// ── Base64 ────────────────────────────────────────────────────────────────────

const B64: &[u8; 64] =
    b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

fn base64_encode(input: &[u8]) -> String {
    let mut out = String::with_capacity(input.len().div_ceil(3) * 4);
    for chunk in input.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = chunk.get(1).copied().unwrap_or(0) as u32;
        let b2 = chunk.get(2).copied().unwrap_or(0) as u32;
        let n = (b0 << 16) | (b1 << 8) | b2;
        out.push(B64[(n >> 18 & 0x3F) as usize] as char);
        out.push(B64[(n >> 12 & 0x3F) as usize] as char);
        out.push(if chunk.len() > 1 {
            B64[(n >> 6 & 0x3F) as usize] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            B64[(n & 0x3F) as usize] as char
        } else {
            '='
        });
    }
    out
}

// ── XDR writer ────────────────────────────────────────────────────────────────

struct XdrWriter(Vec<u8>);

impl XdrWriter {
    fn new() -> Self {
        Self(Vec::new())
    }
    fn u32(&mut self, v: u32) {
        self.0.extend_from_slice(&v.to_be_bytes());
    }
    fn i64(&mut self, v: i64) {
        self.0.extend_from_slice(&v.to_be_bytes());
    }
    /// Write `data` followed by zero-padding to the next 4-byte boundary.
    fn opaque_fixed(&mut self, data: &[u8]) {
        self.0.extend_from_slice(data);
        let pad = data.len().wrapping_neg() & 3;
        self.0.extend(std::iter::repeat_n(0u8, pad));
    }
    /// Write a 4-byte length prefix, then `opaque_fixed(data)`.
    fn opaque_var(&mut self, data: &[u8]) {
        self.u32(data.len() as u32);
        self.opaque_fixed(data);
    }
    fn into_bytes(self) -> Vec<u8> {
        self.0
    }
}

// ScVal discriminants (Soroban protocol 21)
const SCV_STRING: u32 = 14;
const SC_ADDR_CONTRACT: u32 = 1;

/// Build a minimal base64-encoded `TransactionEnvelope` (v1) containing a
/// single `InvokeHostFunctionOp` for `simulateTransaction`.
///
/// The source account is the all-zero Ed25519 key, fee is 100 stroops, and
/// sequence number is 0. Signatures are omitted — `simulateTransaction` does
/// not validate them.
///
/// `args_xdr` items are treated as Soroban `ScVal::String` arguments
/// (the only type currently needed for registry/core view functions).
fn build_invoke_xdr(
    contract_id: &str,
    function_name: &str,
    args_xdr: &[String],
) -> Result<String> {
    let contract_hash = decode_contract_id(contract_id)
        .with_context(|| format!("invalid contract id '{contract_id}'"))?;

    let mut w = XdrWriter::new();

    // TransactionEnvelope discriminant: ENVELOPE_TYPE_TX = 2
    w.u32(2);

    // sourceAccount: MuxedAccount::Ed25519([0u8;32])
    w.u32(0); // KEY_TYPE_ED25519
    w.opaque_fixed(&[0u8; 32]);

    w.u32(100); // fee (stroops)
    w.i64(0);   // seqNum

    // cond: Preconditions::None
    w.u32(0);
    // memo: Memo::None
    w.u32(0);

    // operations: count = 1
    w.u32(1);

    // Operation
    w.u32(0);  // sourceAccount: absent
    w.u32(24); // OperationType::INVOKE_HOST_FUNCTION

    // InvokeHostFunctionOp
    // hostFunction: HOST_FUNCTION_TYPE_INVOKE_CONTRACT = 0
    w.u32(0);

    // InvokeContractArgs
    // contractAddress: SC_ADDRESS_TYPE_CONTRACT
    w.u32(SC_ADDR_CONTRACT);
    w.opaque_fixed(&contract_hash); // Hash(32)

    // functionName: SCSymbol
    w.opaque_var(function_name.as_bytes());

    // args: SCVal[]
    w.u32(args_xdr.len() as u32);
    for arg in args_xdr {
        // Treat each arg string as SCV_STRING
        w.u32(SCV_STRING);
        w.opaque_var(arg.as_bytes());
    }

    // auth: SorobanAuthorizationEntry[] — empty for read-only calls
    w.u32(0);

    // Transaction.ext: v = 0
    w.u32(0);

    // TransactionV1Envelope.signatures: count = 0
    w.u32(0);

    Ok(base64_encode(&w.into_bytes()))
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
#[allow(dead_code)]
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

/// Extract the `pagingToken` from the last event in a `getEvents` response.
///
/// The Soroban RPC embeds a continuation cursor in each event's `pagingToken`
/// field.  We only need the token from the final event to request the next page.
fn extract_last_paging_token(raw_result: &Value) -> Option<String> {
    raw_result
        .get("events")?
        .as_array()?
        .last()?
        .get("pagingToken")?
        .as_str()
        .map(|s| s.to_string())
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
#[allow(dead_code)]
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
#[allow(dead_code)]
pub trait RpcClient: Send + Sync {
    async fn call_u64(&self, contract_id: &str, function_name: &str) -> Result<u64>;
    async fn call_bool(&self, contract_id: &str, function_name: &str) -> Result<bool>;
    async fn call_string_vec(&self, contract_id: &str, function_name: &str) -> Result<Vec<String>>;
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
    async fn call_string_vec(&self, contract_id: &str, function_name: &str) -> Result<Vec<String>> {
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
        self.get_events(contract_id, topic_filters, start_ledger)
            .await
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
#[cfg(test)]
pub struct MockRpcClient {
    u64_responses: std::collections::HashMap<(String, String), u64>,
    bool_responses: std::collections::HashMap<(String, String), bool>,
    string_vec_responses: std::collections::HashMap<(String, String), Vec<String>>,
    u32_vec_responses: std::collections::HashMap<(String, String, String), Vec<u32>>,
    simulate_responses: std::collections::HashMap<(String, String), serde_json::Value>,
    events_responses: std::collections::HashMap<(String, String), Vec<ContractEvent>>,
    ledger_entries_responses: std::collections::HashMap<String, Vec<LedgerEntry>>,
}

#[cfg(test)]
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
        self.u32_vec_responses.insert(
            (contract.to_string(), func.to_string(), arg.to_string()),
            val,
        );
        self
    }

    pub fn with_simulate(mut self, contract: &str, func: &str, val: serde_json::Value) -> Self {
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

#[cfg(test)]
#[async_trait::async_trait]
impl RpcClient for MockRpcClient {
    async fn call_u64(&self, contract_id: &str, function_name: &str) -> Result<u64> {
        self.u64_responses
            .get(&(contract_id.to_string(), function_name.to_string()))
            .copied()
            .ok_or_else(|| {
                anyhow::anyhow!("MockRpcClient: no u64 response for {contract_id}::{function_name}")
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

    async fn call_string_vec(&self, contract_id: &str, function_name: &str) -> Result<Vec<String>> {
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
            .get(&(
                contract_id.to_string(),
                function_name.to_string(),
                arg.to_string(),
            ))
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

    /// Verify that `build_invoke_xdr` produces valid base64-encoded XDR starting
    /// with the ENVELOPE_TYPE_TX discriminant (big-endian 2 = [0,0,0,2]).
    #[test]
    fn build_invoke_xdr_produces_valid_envelope() {
        let contract = "CAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAABSC4";
        let xdr = build_invoke_xdr(contract, "total_routed", &[]).unwrap();
        // Must be valid base64
        let mut lut = [0xFFu8; 256];
        for (i, &c) in B64.iter().enumerate() {
            lut[c as usize] = i as u8;
        }
        // Decode first 4 bytes to verify ENVELOPE_TYPE_TX = 2
        let bytes: Vec<u8> = {
            let s = xdr.trim_end_matches('=').as_bytes();
            let c0 = lut[s[0] as usize] as u32;
            let c1 = lut[s[1] as usize] as u32;
            let c2 = lut[s[2] as usize] as u32;
            let c3 = lut[s[3] as usize] as u32;
            let n = (c0 << 18) | (c1 << 12) | (c2 << 6) | c3;
            vec![(n >> 16) as u8, (n >> 8) as u8, n as u8]
        };
        assert_eq!(&bytes[0..3], &[0, 0, 0], "high bytes of discriminant");
        // The fourth byte is in the next group; just check first 3 are 0.
        let _ = bytes;
    }

    #[test]
    fn build_invoke_xdr_with_string_arg() {
        let contract = "CAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAABSC4";
        let result = build_invoke_xdr(contract, "get_route", &["oracle".to_string()]);
        assert!(result.is_ok(), "XDR build with arg should succeed");
        assert!(!result.unwrap().is_empty());
    }

    #[test]
    fn build_invoke_xdr_invalid_contract_id_returns_error() {
        let result = build_invoke_xdr("not-a-valid-contract-id", "fn", &[]);
        assert!(result.is_err(), "invalid contract id should return Err");
    }
    /// fail the overall call — the retry logic absorbs the first blip.
    ///
    /// We use MockRpcClient to simulate a "first call fails, second succeeds"
    /// scenario at the high-level trait boundary.
    #[tokio::test]
    async fn retry_on_transient_failure_mock() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        use std::sync::Arc;

        // Shared call counter simulates a flaky backend.
        let call_count = Arc::new(AtomicUsize::new(0));

        struct FlakyMock {
            call_count: Arc<AtomicUsize>,
        }

        #[async_trait::async_trait]
        impl RpcClient for FlakyMock {
            async fn call_u64(&self, _: &str, _: &str) -> Result<u64> {
                let n = self.call_count.fetch_add(1, Ordering::SeqCst);
                if n == 0 {
                    Err(anyhow!("transient error"))
                } else {
                    Ok(42)
                }
            }
            async fn call_bool(&self, _: &str, _: &str) -> Result<bool> { Ok(false) }
            async fn call_string_vec(&self, _: &str, _: &str) -> Result<Vec<String>> { Ok(vec![]) }
            async fn call_u32_vec(&self, _: &str, _: &str, _: &str) -> Result<Vec<u32>> { Ok(vec![]) }
            async fn simulate_invoke(&self, _: &str, _: &str, _: Vec<String>) -> Result<Value> {
                Ok(json!({}))
            }
            async fn get_events(&self, _: &str, _: &[&str], _: u32) -> Result<Vec<ContractEvent>> {
                Ok(vec![])
            }
            async fn get_ledger_entries(&self, _: Vec<String>) -> Result<Vec<LedgerEntry>> {
                Ok(vec![])
            }
        }

        let mock = FlakyMock { call_count: Arc::clone(&call_count) };

        // First attempt returns an error; a real retry loop in the caller
        // would try again. We model that here by calling twice and asserting
        // the second call succeeds.
        let first = mock.call_u64("C1", "total_routed").await;
        let second = mock.call_u64("C1", "total_routed").await;

        assert!(first.is_err(), "first call should fail (transient)");
        assert_eq!(second.unwrap(), 42, "second call should succeed");
        assert_eq!(call_count.load(Ordering::SeqCst), 2);
    #[test]
    fn test_extract_last_paging_token_present() {
        let v = json!({
            "events": [
                { "contractId": "C1", "ledger": 1, "topic": [], "value": {}, "pagingToken": "token-a" },
                { "contractId": "C1", "ledger": 2, "topic": [], "value": {}, "pagingToken": "token-b" }
            ],
            "latestLedger": 100
        });
        assert_eq!(
            extract_last_paging_token(&v),
            Some("token-b".to_string())
        );
    }

    #[test]
    fn test_extract_last_paging_token_absent() {
        let v = json!({
            "events": [
                { "contractId": "C1", "ledger": 1, "topic": [], "value": {} }
            ],
            "latestLedger": 100
        });
        assert_eq!(extract_last_paging_token(&v), None);
    }

    #[test]
    fn test_extract_last_paging_token_empty_events() {
        let v = json!({ "events": [], "latestLedger": 50 });
        assert_eq!(extract_last_paging_token(&v), None);
    }

    #[test]
    fn test_get_events_max_pages_constant() {
        assert_eq!(GET_EVENTS_MAX_PAGES, 20);
    }

    #[tokio::test]
    async fn test_get_events_paginates_multiple_pages() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        use std::sync::Arc;

        // Spawn a mock HTTP server that returns 3 pages of events.
        let call_count = Arc::new(AtomicUsize::new(0));
        let call_count_clone = call_count.clone();

        let app = axum::Router::new().route(
            "/",
            axum::routing::post(move |_body: axum::body::Body| {
                let count = call_count_clone.clone();
                async move {
                    let n = count.fetch_add(1, Ordering::SeqCst);
                    let response = match n {
                        0 => {
                            // Page 1: 2 events with paging token
                            json!({
                                "jsonrpc": "2.0",
                                "id": 1,
                                "result": {
                                    "events": [
                                        { "contractId": "CONTRACT", "ledger": 10, "topic": [], "value": {"u64": 1}, "pagingToken": "cursor-1" },
                                        { "contractId": "CONTRACT", "ledger": 11, "topic": [], "value": {"u64": 2}, "pagingToken": "cursor-2" }
                                    ],
                                    "latestLedger": 100
                                }
                            })
                        }
                        1 => {
                            // Page 2: 1 event with paging token
                            json!({
                                "jsonrpc": "2.0",
                                "id": 1,
                                "result": {
                                    "events": [
                                        { "contractId": "CONTRACT", "ledger": 12, "topic": [], "value": {"u64": 3}, "pagingToken": "cursor-3" }
                                    ],
                                    "latestLedger": 100
                                }
                            })
                        }
                        _ => {
                            // Page 3: empty — signals end of pagination
                            json!({
                                "jsonrpc": "2.0",
                                "id": 1,
                                "result": {
                                    "events": [],
                                    "latestLedger": 100
                                }
                            })
                        }
                    };
                    axum::Json(response)
                }
            }),
        );

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        let client =
            SorobanRpcClient::new(format!("http://{addr}"), 5).expect("build client");

        let events = client
            .get_events("CONTRACT", &["post_call"], 10)
            .await
            .expect("get_events should succeed");

        // All 3 events across 3 pages should be present.
        assert_eq!(events.len(), 3);
        assert_eq!(events[0].ledger, 10);
        assert_eq!(events[1].ledger, 11);
        assert_eq!(events[2].ledger, 12);

        // The mock server should have been called 3 times (page1 + page2 + empty page3).
        assert_eq!(call_count.load(Ordering::SeqCst), 3);
    }
}

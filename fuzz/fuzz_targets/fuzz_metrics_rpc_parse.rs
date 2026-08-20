#![no_main]

use libfuzzer_sys::fuzz_target;
use serde_json::Value;

// Fuzz the metrics crate's rpc parsing functions with arbitrary input.
//
// The strkey decoder now lives in the shared `router-off-chain-common::xdr`
// module; this target exercises the metrics call path through
// `build_invoke_xdr_from_strkey` as well as the shared decoder directly.
// `decode_contract_id` accepts a `&str`, so we restrict to valid UTF-8.
// The JSON extraction functions accept arbitrary `serde_json::Value`.
// None of these should panic regardless of input.
fuzz_target!(|data: &[u8]| {
    // Fuzz the shared strkey decoder with arbitrary strings.
    if let Ok(s) = std::str::from_utf8(data) {
        let _ = router_off_chain_common::xdr::decode_contract_id(s);
    }

    // Fuzz the metrics call path: envelope building from a strkey contract id.
    if let Ok(s) = std::str::from_utf8(data) {
        let args = [s.to_string()];
        let _ = router_off_chain_common::xdr::build_invoke_xdr_from_strkey(s, "fn", &args);
    }

    // Fuzz JSON extraction with arbitrary bytes parsed as JSON.
    if let Ok(v) = serde_json::from_slice::<Value>(data) {
        let _ = router_metrics_exporter::rpc::extract_u64_from_sim_result(&v);
        let _ = router_metrics_exporter::rpc::extract_bool_from_sim_result(&v);
        let _ = router_metrics_exporter::rpc::extract_string_vec_from_sim_result(&v);
        let _ = router_metrics_exporter::rpc::extract_u32_vec_from_sim_result(&v);
        let _ = router_metrics_exporter::rpc::extract_last_paging_token(&v);
    }
});
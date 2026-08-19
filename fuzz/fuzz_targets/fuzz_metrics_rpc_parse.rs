#![no_main]

use libfuzzer_sys::fuzz_target;
use serde_json::Value;

// Fuzz the metrics crate's rpc parsing functions with arbitrary input.
//
// `decode_contract_id` accepts a `&str`, so we restrict to valid UTF-8.
// The JSON extraction functions accept arbitrary `serde_json::Value`.
// None of these should panic regardless of input.
fuzz_target!(|data: &[u8]| {
    // Fuzz the strkey decoder with arbitrary strings.
    if let Ok(s) = std::str::from_utf8(data) {
        let _ = router_metrics_exporter::rpc::decode_contract_id(s);
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

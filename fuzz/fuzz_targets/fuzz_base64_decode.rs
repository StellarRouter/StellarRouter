#![no_main]

use libfuzzer_sys::fuzz_target;
use router_api_server::xdr::{base64_decode, base64_encode};

// Fuzz the base64 decoder with arbitrary string input and verify the
// encode→decode round-trip is stable.
//
// `base64_decode` must never panic regardless of input.
fuzz_target!(|data: &[u8]| {
    // Arbitrary UTF-8 strings (including malformed base64).
    if let Ok(s) = std::str::from_utf8(data) {
        let _ = base64_decode(s);
    }

    // Round-trip: encoding is infallible, so decoding the result must succeed.
    let encoded = base64_encode(data);
    let decoded = base64_decode(&encoded).expect("round-trip decode must not fail");
    assert_eq!(decoded, data, "round-trip must be lossless");
});

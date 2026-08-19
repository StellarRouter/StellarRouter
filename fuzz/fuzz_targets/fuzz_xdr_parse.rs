#![no_main]

use libfuzzer_sys::fuzz_target;
use router_api_server::xdr::{base64_encode, parse_route_entry, parse_string_vec};

// Feed arbitrary bytes through the two XDR response parsers.
//
// Both functions accept a base64-encoded XDR blob, so we first base64-encode
// the raw fuzzer input (guaranteeing a valid base64 string) and then call the
// parsers.  They are expected to return `Ok` or `Err` — never panic.
fuzz_target!(|data: &[u8]| {
    let b64 = base64_encode(data);
    let _ = parse_string_vec(&b64);
    let _ = parse_route_entry(&b64);

    // Also exercise the parsers with input that is already a string (e.g. a
    // base64 payload the fuzzer synthesises directly as UTF-8 text).
    if let Ok(s) = std::str::from_utf8(data) {
        let _ = parse_string_vec(s);
        let _ = parse_route_entry(s);
    }
});

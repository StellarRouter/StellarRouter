#![no_main]

use libfuzzer_sys::fuzz_target;
use router_off_chain_common::xdr::{
    base64_encode, build_invoke_xdr, parse_route_entry, parse_string_vec,
};

// Feed arbitrary bytes through the shared XDR response parsers and the
// api-server transaction-envelope builder.
//
// The two response parsers accept a base64-encoded XDR blob, so we first
// base64-encode the raw fuzzer input (guaranteeing a valid base64 string) and
// then call the parsers.  They are expected to return `Ok` or `Err` — never
// panic. The envelope builder is also exercised so the shared XDR writer is
// fuzzed with arbitrary function names / string arguments.
fuzz_target!(|data: &[u8]| {
    let b64 = base64_encode(data);
    let _ = parse_string_vec(&b64);
    let _ = parse_route_entry(&b64);

    // Exercise the api-server call path: a hash-based envelope build with
    // arbitrary function names and string args.
    let function: String = String::from_utf8_lossy(data).into_owned();
    let args = [router_off_chain_common::xdr::ScArg::String(&function)];
    build_invoke_xdr(&[0u8; 32], &function, &args);

    // Also exercise the parsers with input that is already a string (e.g. a
    // base64 payload the fuzzer synthesises directly as UTF-8 text).
    if let Ok(s) = std::str::from_utf8(data) {
        let _ = parse_string_vec(s);
        let _ = parse_route_entry(s);
    }
});
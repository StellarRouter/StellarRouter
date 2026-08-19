#![no_main]

use libfuzzer_sys::fuzz_target;
use router_api_server::xdr::decode_contract_id;

// Feed arbitrary byte sequences to the strkey decoder.
//
// `decode_contract_id` accepts a `&str`, so we restrict the fuzzer corpus to
// valid UTF-8.  Any input — including correctly-checksummed strkeys, wrong
// version bytes, bad base32 characters, or wrong lengths — must not panic.
fuzz_target!(|data: &[u8]| {
    if let Ok(s) = std::str::from_utf8(data) {
        let _ = decode_contract_id(s);
    }
});

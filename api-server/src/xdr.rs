//! XDR/strkey/base64 parsing utilities, re-exported from the shared
//! [`router_off_chain_common::xdr`] module.
//!
//! These helpers were historically implemented (and fuzzed) here before being
//! consolidated into `router-off-chain-common` so that both the API server and
//! the metrics exporter share a single implementation. This module is kept as a
//! thin re-export so existing `crate::xdr::…` call sites remain unchanged.
//!
//! The full shared surface (base64 encode/decode, account/contract strkey
//! encoding, `build_invoke_xdr_from_strkey`, `ParsedRouteEntry`, …) lives in
//! `router_off_chain_common::xdr`; only the names this crate actually uses are
//! re-exported here.

pub use router_off_chain_common::xdr::{
    build_invoke_xdr, decode_contract_id, parse_route_entry, parse_string_vec, ScArg,
};
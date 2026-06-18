//! Re-exports the shared input validation helpers from `router-off-chain-common`.
//!
//! See [`router_off_chain_common::validation`] for full documentation.

pub use router_off_chain_common::validation::{
    validate_contract_id, validate_listen_addr, validate_route_name, validate_scrape_interval,
};

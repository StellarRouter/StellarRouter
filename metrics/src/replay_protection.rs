//! Re-exports the shared replay-protection middleware from `router-off-chain-common`.
//!
//! See [`router_off_chain_common::replay_protection`] for full documentation.

pub use router_off_chain_common::replay_protection::{
    replay_protection_middleware, NonceCache, ReplayProtectionConfig,
};

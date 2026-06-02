//! CLI argument / environment variable configuration.

use clap::Parser;

/// Prometheus metrics exporter for the stellar-router suite.
///
/// All flags can also be set via environment variables (shown in brackets).
#[derive(Debug, Clone, Parser)]
#[command(
    name = "router-metrics-exporter",
    about = "Exposes stellar-router on-chain metrics in Prometheus format",
    version
)]
pub struct Args {
    /// Soroban RPC endpoint URL.
    ///
    /// Example: `https://soroban-testnet.stellar.org`
    #[arg(
        long,
        env = "ROUTER_RPC_URL",
        default_value = "https://soroban-testnet.stellar.org"
    )]
    pub rpc_url: String,

    /// Stellar network passphrase (used to decode XDR correctly).
    ///
    /// Defaults to the public testnet passphrase.
    #[arg(
        long,
        env = "ROUTER_NETWORK_PASSPHRASE",
        default_value = "Test SDF Network ; September 2015"
    )]
    pub network_passphrase: String,

    /// Contract ID of the deployed `router-core` contract.
    ///
    /// Leave empty to skip scraping this contract.
    #[arg(long, env = "ROUTER_CORE_CONTRACT_ID", default_value = "")]
    pub core_contract_id: String,

    /// Contract ID of the deployed `router-middleware` contract.
    ///
    /// Leave empty to skip scraping this contract.
    #[arg(long, env = "ROUTER_MIDDLEWARE_CONTRACT_ID", default_value = "")]
    pub middleware_contract_id: String,

    /// Contract ID of the deployed `router-registry` contract.
    ///
    /// Leave empty to skip scraping this contract.
    #[arg(long, env = "ROUTER_REGISTRY_CONTRACT_ID", default_value = "")]
    pub registry_contract_id: String,

    /// Contract ID of the deployed `router-quote` contract.
    ///
    /// Leave empty to skip scraping this contract.
    #[arg(long, env = "ROUTER_QUOTE_CONTRACT_ID", default_value = "")]
    pub quote_contract_id: String,

    /// Contract ID of the deployed `router-execution` contract.
    ///
    /// Leave empty to skip scraping this contract.
    #[arg(long, env = "ROUTER_EXECUTION_CONTRACT_ID", default_value = "")]
    pub execution_contract_id: String,

    /// How often (in seconds) to poll the Soroban RPC for fresh data.
    #[arg(long, env = "ROUTER_SCRAPE_INTERVAL_SECS", default_value_t = 15)]
    pub scrape_interval_secs: u64,

    /// Address and port to listen on for the `/metrics` HTTP endpoint.
    #[arg(long, env = "ROUTER_LISTEN", default_value = "0.0.0.0:9090")]
    pub listen: String,

    /// RPC request timeout in seconds.
    #[arg(long, env = "ROUTER_RPC_TIMEOUT_SECS", default_value_t = 10)]
    pub rpc_timeout_secs: u64,
}

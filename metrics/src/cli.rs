//! CLI argument / environment variable configuration.

use clap::{Parser, ValueEnum};

/// Event ingestion mode: poll the Soroban RPC on a fixed interval, or subscribe
/// to the Stellar Horizon SSE event stream for near-real-time updates.
///
/// **poll** (default) — The original behaviour. The exporter calls
/// `simulateTransaction` / `getEvents` every `scrape_interval_secs` seconds.
/// Reliable and works with any Soroban RPC endpoint.
///
/// **sse** — Subscribe to the Horizon `/contracts/{id}/events` SSE endpoint
/// for each configured contract. Metrics are updated as events arrive, giving
/// sub-second latency. Automatic reconnect with exponential backoff is built in.
/// Polling mode is still used as a fallback when the SSE connection is
/// unavailable.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum, Default)]
pub enum EventMode {
    /// Poll the Soroban RPC on a fixed interval (original behaviour).
    #[default]
    Poll,
    /// Subscribe to the Stellar Horizon SSE endpoint for near-real-time events.
    Sse,
}

impl std::fmt::Display for EventMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            EventMode::Poll => write!(f, "poll"),
            EventMode::Sse => write!(f, "sse"),
        }
    }
}

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

    /// Contract ID of the deployed `router-access` contract.
    ///
    /// Leave empty to skip scraping role/blacklist metrics from this contract.
    #[arg(long, env = "ROUTER_ACCESS_CONTRACT_ID", default_value = "")]
    pub access_contract_id: String,

    /// Contract ID of the deployed `router-timelock` contract.
    ///
    /// Leave empty to skip scraping timelock metrics from this contract.
    #[arg(long, env = "ROUTER_TIMELOCK_CONTRACT_ID", default_value = "")]
    pub timelock_contract_id: String,

    /// Contract ID of the deployed `router-multicall` contract.
    ///
    /// Leave empty to skip scraping multicall metrics from this contract.
    #[arg(long, env = "ROUTER_MULTICALL_CONTRACT_ID", default_value = "")]
    pub multicall_contract_id: String,

    /// How often (in seconds) to poll the Soroban RPC for fresh data.
    ///
    /// Used in `poll` mode. In `sse` mode this value is also used for the
    /// initial bootstrap scrape and as the reconnect baseline interval.
    #[arg(long, env = "ROUTER_SCRAPE_INTERVAL_SECS", default_value_t = 15)]
    pub scrape_interval_secs: u64,

    /// Address and port to listen on for the `/metrics` HTTP endpoint.
    #[arg(long, env = "ROUTER_LISTEN", default_value = "0.0.0.0:9090")]
    pub listen: String,

    /// RPC request timeout in seconds.
    #[arg(long, env = "ROUTER_RPC_TIMEOUT_SECS", default_value_t = 10)]
    pub rpc_timeout_secs: u64,

    // ── SSE / event-mode configuration ────────────────────────────────────────

    /// Event ingestion mode: `poll` (default) or `sse`.
    ///
    /// `poll` — scrape the Soroban RPC every `scrape_interval_secs` seconds
    /// (original behaviour, always available).
    ///
    /// `sse` — subscribe to the Stellar Horizon SSE event stream for
    /// near-real-time metric updates. Falls back to a poll-based bootstrap
    /// when SSE is unavailable and automatically reconnects on disconnect.
    #[arg(
        long,
        env = "ROUTER_EVENT_MODE",
        value_enum,
        default_value_t = EventMode::Poll
    )]
    pub event_mode: EventMode,

    /// Base URL of the Stellar Horizon server used for SSE subscriptions.
    ///
    /// Only used when `--event-mode sse` is set.
    ///
    /// Example: `https://horizon-testnet.stellar.org`
    #[arg(
        long,
        env = "ROUTER_HORIZON_URL",
        default_value = "https://horizon-testnet.stellar.org"
    )]
    pub horizon_url: String,

    /// Maximum number of SSE reconnect attempts before giving up and falling
    /// back to poll mode.
    ///
    /// Set to 0 for unlimited retries. Only used when `--event-mode sse`.
    #[arg(long, env = "ROUTER_SSE_MAX_RECONNECTS", default_value_t = 10)]
    pub sse_max_reconnects: u32,

    /// Base reconnect delay in milliseconds for the SSE subscriber.
    ///
    /// The actual delay is `sse_reconnect_delay_ms * 2^attempt`, capped at
    /// `sse_reconnect_max_delay_ms`. Only used when `--event-mode sse`.
    #[arg(long, env = "ROUTER_SSE_RECONNECT_DELAY_MS", default_value_t = 1000)]
    pub sse_reconnect_delay_ms: u64,

    /// Maximum reconnect delay in milliseconds for the SSE subscriber.
    ///
    /// Caps the exponential back-off ceiling. Only used when `--event-mode sse`.
    #[arg(
        long,
        env = "ROUTER_SSE_RECONNECT_MAX_DELAY_MS",
        default_value_t = 30_000
    )]
    pub sse_reconnect_max_delay_ms: u64,
}

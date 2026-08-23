# router-metrics-exporter

**Prometheus/OpenTelemetry metrics exporter for the stellar-router suite.**

## Overview

Soroban smart contracts run inside the Stellar network as WASM and cannot open sockets or push metrics themselves. This binary bridges the gap by:

1. Polling the Soroban RPC endpoint at a configurable interval
2. Reading on-chain state from each router contract (total_routed, total_calls, circuit-breaker state, paused flags, etc.)
3. Exposing a `/metrics` HTTP endpoint in the Prometheus text format

## Metrics Exposed

| Metric | Type | Labels | Description |
|--------|------|--------|-------------|
| `router_core_total_routed` | Gauge | `contract` | Cumulative successful route resolutions |
| `router_core_paused` | Gauge | `contract` | 1 if the router is globally paused |
| `router_core_route_paused` | Gauge | `contract`, `route` | 1 if a specific route is paused |
| `router_middleware_total_calls` | Gauge | `contract` | Cumulative pre-call invocations |
| `router_middleware_circuit_open` | Gauge | `contract`, `route` | 1 if the circuit breaker is open |
| `router_middleware_failure_count` | Gauge | `contract`, `route` | Consecutive failure count |
| `router_registry_total_names` | Gauge | `contract` | Total contract names registered |
| `router_quote_total_generated` | Counter | `contract` | Cumulative `quote_generated` events |
| `router_quote_total_fee_estimated` | Counter | `contract` | Cumulative `fee_estimated` events |
| `router_execution_total_executions` | Counter | `contract` | Cumulative executions recorded |
| `router_execution_total_errors` | Counter | `contract` | Cumulative execution errors recorded |
| `router_execution_max_retries` | Gauge | `contract` | Configured max retries |
| `router_access_role_member_count` | Gauge | `contract`, `role` | Indexed members ever added per role (`get_role_count`) |
| `router_access_blacklist_size` | Gauge | `contract` | Distinct addresses on the blacklist (`get_blacklist_count`) |
| `router_timelock_pending_operations` | Gauge | `contract` | Pending time-locked operations (`get_pending_op_count`) |
| `router_multicall_total_batches` | Gauge | `contract` | Total batches submitted (`total_batches`) |
| `router_multicall_batch_success_total` | Counter | `contract` | Cumulative successful calls (`call_result` events) |
| `router_multicall_batch_failure_total` | Counter | `contract` | Cumulative failed calls (`call_result` events) |
| `router_scrape_duration_seconds` | Histogram | `contract` | Time spent scraping each contract |
| `router_scrape_errors_total` | Counter | `contract` | Number of failed scrape attempts |
| `router_up` | Gauge | — | 1 if the last scrape cycle succeeded |
| `router_sse_connected` | Gauge | `contract` | 1 if the SSE stream is active (SSE mode only) |
| `router_sse_reconnects_total` | Counter | `contract` | Total SSE reconnect attempts (SSE mode only) |
| `router_sse_events_total` | Counter | `contract` | Total SSE events received (SSE mode only) |

## Installation

### From source

```bash
cd metrics
cargo build --release
```

The binary will be at `target/release/router-metrics-exporter`.

### Docker (optional)

```dockerfile
FROM rust:1.83-slim as builder
WORKDIR /build
COPY . .
RUN cargo build --release -p router-metrics-exporter

FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y ca-certificates && rm -rf /var/lib/apt/lists/*
COPY --from=builder /build/target/release/router-metrics-exporter /usr/local/bin/
ENTRYPOINT ["router-metrics-exporter"]
```

## Usage

### Command-line flags

All flags can also be set via environment variables (shown in brackets).

```
router-metrics-exporter [OPTIONS]

Options:
  --rpc-url <URL>
      Soroban RPC endpoint URL
      [env: ROUTER_RPC_URL]
      [default: https://soroban-testnet.stellar.org]

  --rpc-urls <URLS>...
      Soroban RPC endpoint URLs for failover (comma-separated or repeat flag)
      [env: ROUTER_RPC_URLS]

  --network-passphrase <PASSPHRASE>
      Stellar network passphrase (used to decode XDR correctly)
      [env: ROUTER_NETWORK_PASSPHRASE]
      [default: Test SDF Network ; September 2015]

  --core-contract-id <CONTRACT_ID>
      Contract ID of the deployed router-core contract
      [env: ROUTER_CORE_CONTRACT_ID]
      [default: ]

  --middleware-contract-id <CONTRACT_ID>
      Contract ID of the deployed router-middleware contract
      [env: ROUTER_MIDDLEWARE_CONTRACT_ID]
      [default: ]

  --registry-contract-id <CONTRACT_ID>
      Contract ID of the deployed router-registry contract
      [env: ROUTER_REGISTRY_CONTRACT_ID]
      [default: ]

  --access-contract-id <CONTRACT_ID>
      Contract ID of the deployed router-access contract
      [env: ROUTER_ACCESS_CONTRACT_ID]
      [default: ]

  --timelock-contract-id <CONTRACT_ID>
      Contract ID of the deployed router-timelock contract
      [env: ROUTER_TIMELOCK_CONTRACT_ID]
      [default: ]

  --multicall-contract-id <CONTRACT_ID>
      Contract ID of the deployed router-multicall contract
      [env: ROUTER_MULTICALL_CONTRACT_ID]
      [default: ]

  --scrape-interval-secs <SECONDS>
      How often (in seconds) to poll the Soroban RPC for fresh data
      [env: ROUTER_SCRAPE_INTERVAL_SECS]
      [default: 15]

  --listen <ADDRESS>
      Address and port to listen on for the /metrics HTTP endpoint
      [env: ROUTER_LISTEN]
      [default: 0.0.0.0:9090]

  --rpc-timeout-secs <SECONDS>
      RPC request timeout in seconds
      [env: ROUTER_RPC_TIMEOUT_SECS]
      [default: 10]

  --event-mode <MODE>
      Event ingestion mode: poll (default) or sse
      [env: ROUTER_EVENT_MODE]
      [default: poll]
      [possible values: poll, sse]

  --horizon-url <URL>
      Stellar Horizon base URL for SSE subscriptions (sse mode only)
      [env: ROUTER_HORIZON_URL]
      [default: https://horizon-testnet.stellar.org]

  --sse-max-reconnects <N>
      Maximum SSE reconnect attempts before giving up (0 = unlimited)
      [env: ROUTER_SSE_MAX_RECONNECTS]
      [default: 10]

  --sse-reconnect-delay-ms <MS>
      Base reconnect back-off delay in milliseconds (doubles each attempt)
      [env: ROUTER_SSE_RECONNECT_DELAY_MS]
      [default: 1000]

  --sse-reconnect-max-delay-ms <MS>
      Maximum reconnect back-off delay in milliseconds
      [env: ROUTER_SSE_RECONNECT_MAX_DELAY_MS]
      [default: 30000]

  --max-cardinality <N>
      Maximum distinct label values per high-cardinality metric
      (per-route, per-name). Overflow values are grouped into _other.
      [env: ROUTER_MAX_CARDINALITY]
      [default: 100]

  -h, --help
      Print help

  -V, --version
      Print version
```

## Event Modes

The exporter supports two event ingestion modes, selectable via `--event-mode` or
`ROUTER_EVENT_MODE`.

### Poll mode (default)

```bash
export ROUTER_EVENT_MODE=poll          # or omit — poll is the default
export ROUTER_SCRAPE_INTERVAL_SECS=15  # how often to scrape
```

The exporter calls `simulateTransaction` / `getEvents` on the Soroban RPC every
`scrape_interval_secs` seconds. This is the original behaviour and works with any
Soroban RPC endpoint without needing a Horizon server.

### SSE mode

```bash
export ROUTER_EVENT_MODE=sse
export ROUTER_HORIZON_URL=https://horizon-testnet.stellar.org
export ROUTER_SSE_MAX_RECONNECTS=10       # 0 = unlimited
export ROUTER_SSE_RECONNECT_DELAY_MS=1000
export ROUTER_SSE_RECONNECT_MAX_DELAY_MS=30000
```

In SSE mode the exporter:

1. Performs a **bootstrap poll** at startup (same as poll mode) so all state-based
   metrics (total_routed, circuit breaker state, etc.) are populated immediately.
2. Spawns one SSE subscriber per configured contract that connects to the
   `GET /events?contractId={id}&cursor=now` Horizon endpoint.
3. Updates event-based metrics (quote_generated, post_call, execution_result, …)
   in near-real-time as events arrive — no waiting for the next scrape interval.
4. **Automatically reconnects** with exponential back-off if the stream drops.

The reconnect back-off follows `delay = min(base * 2^attempt, max_delay)`.
After `sse_max_reconnects` failed attempts (when > 0), the subscriber exits and
`router_sse_connected` stays at 0 — state-based metrics (populated by the
bootstrap poll) remain visible.

#### SSE health Prometheus queries

```promql
# Is the SSE stream connected?
router_sse_connected{contract="CBGTG..."}

# Reconnect rate (spikes indicate network instability)
rate(router_sse_reconnects_total{contract="CBGTG..."}[5m])

# Event throughput
rate(router_sse_events_total{contract="CBGTG..."}[1m])
```

### RPC Failover

```bash
export ROUTER_RPC_URLS="https://primary.example.com,https://secondary.example.com"
export ROUTER_SCRAPE_INTERVAL_SECS=15
```

When multiple RPC endpoints are provided, the exporter automatically fails over to the next endpoint if the current one becomes unreachable. Each endpoint gets its own retry budget (controlled by `ROUTER_RPC_MAX_RETRIES` and `ROUTER_RPC_BACKOFF_MS`). If all endpoints fail, the scrape cycle is marked as failed and `router_up` is set to `0`.

The exporter treats each failed endpoint as a health-check failure and moves traffic to healthy endpoints without manual intervention.

### Example: Testnet deployment

```bash
export ROUTER_RPC_URL="https://soroban-testnet.stellar.org"
export ROUTER_CORE_CONTRACT_ID="CBGTG...YOUR_CONTRACT_ID"
export ROUTER_MIDDLEWARE_CONTRACT_ID="CBGTG...YOUR_CONTRACT_ID"
export ROUTER_REGISTRY_CONTRACT_ID="CBGTG...YOUR_CONTRACT_ID"
export ROUTER_ACCESS_CONTRACT_ID="CBGTG...YOUR_CONTRACT_ID"
export ROUTER_TIMELOCK_CONTRACT_ID="CBGTG...YOUR_CONTRACT_ID"
export ROUTER_MULTICALL_CONTRACT_ID="CBGTG...YOUR_CONTRACT_ID"
export ROUTER_SCRAPE_INTERVAL_SECS=30
export ROUTER_LISTEN="0.0.0.0:9090"

./target/release/router-metrics-exporter
```

### Example: Mainnet deployment

```bash
export ROUTER_RPC_URL="https://soroban-mainnet.stellar.org"
export ROUTER_NETWORK_PASSPHRASE="Public Global Stellar Network ; September 2015"
export ROUTER_CORE_CONTRACT_ID="CBGTG...YOUR_CONTRACT_ID"
export ROUTER_MIDDLEWARE_CONTRACT_ID="CBGTG...YOUR_CONTRACT_ID"
export ROUTER_ACCESS_CONTRACT_ID="CBGTG...YOUR_CONTRACT_ID"
export ROUTER_TIMELOCK_CONTRACT_ID="CBGTG...YOUR_CONTRACT_ID"
export ROUTER_MULTICALL_CONTRACT_ID="CBGTG...YOUR_CONTRACT_ID"
export ROUTER_SCRAPE_INTERVAL_SECS=60

./target/release/router-metrics-exporter
```

## Prometheus Configuration

Add the exporter as a scrape target in your `prometheus.yml`:

```yaml
scrape_configs:
  - job_name: 'stellar-router'
    scrape_interval: 30s
    static_configs:
      - targets: ['localhost:9090']
        labels:
          environment: 'testnet'
          service: 'stellar-router'
```

## Grafana Dashboard

Example queries for a Grafana dashboard:

### Route resolution throughput (per minute)

```promql
rate(router_core_total_routed{contract="CBGTG..."}[5m]) * 60
```

### Circuit breaker status

```promql
router_middleware_circuit_open{contract="CBGTG...", route="oracle/get_price"}
```

### Scrape health

```promql
router_up
```

### P95 scrape latency

```promql
histogram_quantile(0.95, rate(router_scrape_duration_seconds_bucket[5m]))
```

### Error rate

```promql
rate(router_scrape_errors_total[5m])
```

## Architecture

### Scraping Strategy

The exporter calls view functions on each contract via `simulateTransaction`:

- **router-core**: `total_routed()`, `get_all_routes()`, `get_route(name)` for each route
- **router-middleware**: `total_calls()`, `get_configured_routes()`, `circuit_breaker_state(route)` for each route
- **router-registry**: `get_all_names()` (total count)

Each contract scrape is timed and any error increments the `router_scrape_errors_total` counter.

### Performance Considerations

- **Scrape interval**: Default 15s. Increase for mainnet (30-60s) to reduce RPC load.
- **RPC timeout**: Default 10s. Increase if you see frequent timeout errors.
- **Overhead**: Minimal — the exporter makes 1-3 RPC calls per contract per scrape cycle.

### Limitations

- **No transaction-level latency**: The exporter tracks scrape latency (off-chain polling time), not on-chain transaction latency. For transaction-level metrics, use Stellar Horizon's transaction history API.
- **Poll mode latency**: In poll mode, metrics are updated on the scrape interval (default 15s). Enable SSE mode (`ROUTER_EVENT_MODE=sse`) for near-real-time event updates.
- **XDR encoding**: The current implementation uses JSON-RPC simulation results. For production deployments with complex data types, integrate the `stellar-xdr` crate for proper XDR encoding/decoding.
- **SSE state-based metrics**: In SSE mode, state-based metrics (total_routed, circuit breaker state) are only updated during the bootstrap poll and on reconnect — not continuously like event-based metrics. Run both modes simultaneously (poll for state, SSE for events) if you need continuous state freshness.

## Metric Cardinality Limits

Prometheus label cardinality explosion is a well-known operational hazard. Metrics that carry per-route or per-name labels (`router_core_route_paused`, `router_middleware_circuit_open`, `router_middleware_failure_count`, `router_middleware_route_calls_total`, `router_middleware_route_failures_total`, `router_registry_version_count`) can grow unboundedly if new routes/names are created on-chain.

The exporter includes a **cardinality limiter** that caps the number of distinct label values tracked per metric. When the cap is exceeded, new values are redirected to a single `_other` bucket, preventing unbounded memory growth.

### Configuration

| Flag | Env Var | Default | Description |
|------|---------|---------|-------------|
| `--max-cardinality` | `ROUTER_MAX_CARDINALITY` | `100` | Max distinct label values per high-cardinality metric |

### Affected Metrics

The following metrics are subject to cardinality limiting:

| Metric | Label Being Limited |
|--------|---------------------|
| `router_core_route_paused` | `route` |
| `router_middleware_circuit_open` | `route` |
| `router_middleware_failure_count` | `route` |
| `router_middleware_route_calls_total` | `route` |
| `router_middleware_route_failures_total` | `route` |
| `router_registry_version_count` | `name` |

### Behavior When Cap Is Exceeded

1. The first N distinct label values (default: 100) are tracked normally as separate Prometheus time-series.
2. Any additional distinct label values beyond the cap are **all mapped to `_other`**.
3. The `_other` bucket accumulates the values (gauges are set, counters are incremented) from all overflow labels.
4. The `contract` label is **never** subject to limiting — contract IDs are deployment-controlled and bounded.

### Example

```bash
# Set a higher cardinality limit (e.g., 500 routes per contract)
export ROUTER_MAX_CARDINALITY=500
./target/release/router-metrics-exporter

# Or via CLI flag
./target/release/router-metrics-exporter --max-cardinality 500
```

### Prometheus Queries for Overflow Monitoring

You can monitor cardinality and detect overflow in Prometheus:

```promql
# Count distinct route label values for a metric
count(count by (route) (router_core_route_paused{contract="CBGTG..."}))

# Check if the _other bucket has any values (indicates overflow)
router_core_route_paused{contract="CBGTG...", route="_other"}
```

## OpenTelemetry Support

The exporter exposes Prometheus metrics. To send metrics to an OpenTelemetry collector:

1. Use the [Prometheus receiver](https://github.com/open-telemetry/opentelemetry-collector-contrib/tree/main/receiver/prometheusreceiver) in your OTel collector config:

```yaml
receivers:
  prometheus:
    config:
      scrape_configs:
        - job_name: 'stellar-router'
          scrape_interval: 30s
          static_configs:
            - targets: ['router-metrics-exporter:9090']

exporters:
  otlp:
    endpoint: "otel-collector:4317"

service:
  pipelines:
    metrics:
      receivers: [prometheus]
      exporters: [otlp]
```

2. Or use the [Prometheus remote write exporter](https://prometheus.io/docs/prometheus/latest/configuration/configuration/#remote_write) to send to an OTel-compatible backend (e.g., Grafana Cloud, Datadog, New Relic).

## Troubleshooting

### "RPC error -32601: Method not found"

The contract may not expose the view function being called. Verify the contract is deployed and initialized:

```bash
stellar contract invoke --id <CONTRACT_ID> --network testnet -- total_routed
```

### "failed to parse JSON-RPC response"

The RPC endpoint may be down or rate-limiting your requests. Check:
- RPC endpoint is reachable: `curl https://soroban-testnet.stellar.org`
- Increase `--rpc-timeout-secs` if requests are timing out
- Increase `--scrape-interval-secs` to reduce request rate

### "router_up" is 0

At least one contract scrape failed. Check logs for details:

```bash
RUST_LOG=router_metrics_exporter=debug ./router-metrics-exporter
```

### High scrape latency

- Reduce the number of routes/configured routes being scraped
- Increase `--scrape-interval-secs` to reduce load
- Use a dedicated RPC endpoint (not the public one) for production

## Development

### Run tests

```bash
cargo test -p router-metrics-exporter
```

### Run locally

```bash
cargo run -p router-metrics-exporter -- \
  --core-contract-id "CBGTG..." \
  --scrape-interval-secs 10
```

### Enable debug logging

```bash
RUST_LOG=router_metrics_exporter=debug cargo run -p router-metrics-exporter
```

## License

MIT (same as the parent stellar-router project)

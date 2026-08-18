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

  -h, --help
      Print help

  -V, --version
      Print version
```

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
- **No real-time events**: Metrics are updated on the scrape interval (default 15s), not in real-time. For real-time monitoring, consider streaming Stellar ledger events via Horizon.
- **XDR encoding**: The current implementation uses JSON-RPC simulation results. For production deployments with complex data types, integrate the `stellar-xdr` crate for proper XDR encoding/decoding.

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

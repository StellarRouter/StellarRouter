# Deployment Guide

Step-by-step instructions for deploying the stellar-router suite to Stellar testnet or mainnet.

---

## Prerequisites

| Tool | Version | Install |
|---|---|---|
| Rust | stable | `rustup install stable` |
| wasm32 target | — | `rustup target add wasm32-unknown-unknown` |
| Stellar CLI | latest | `cargo install --locked stellar-cli` |
| Funded account | — | See [Friendbot](#funding-a-testnet-account) |

---

## Testnet vs Mainnet

| | Testnet | Mainnet |
|---|---|---|
| Network passphrase | `Test SDF Network ; September 2015` | `Public Global Stellar Network ; September 2015` |
| RPC URL | `https://soroban-testnet.stellar.org` | `https://mainnet.stellar.validationcloud.io/v1/<key>` |
| Fund account | Friendbot (free) | Real XLM required |
| Risk | None | Real funds at stake |
| Recommended for | Development, testing | Production only |

**Always deploy and test on testnet before mainnet.**

---

## Funding a Testnet Account

```bash
# Generate a new keypair
stellar keys generate --global admin --network testnet

# Fund via Friendbot
stellar keys fund admin --network testnet

# Verify balance
stellar account show admin --network testnet
```

---

## Build WASM Artifacts

```bash
cargo build --target wasm32-unknown-unknown --release
```

Artifacts will be at:
```
target/wasm32-unknown-unknown/release/router_core.wasm
target/wasm32-unknown-unknown/release/router_registry.wasm
target/wasm32-unknown-unknown/release/router_access.wasm
target/wasm32-unknown-unknown/release/router_middleware.wasm
target/wasm32-unknown-unknown/release/router_timelock.wasm
target/wasm32-unknown-unknown/release/router_multicall.wasm
```

---

## Deployment Order

Deploy in this order. Each contract is independent but the initialization
order matters for your integration:

```
1. router-registry   (no dependencies)
2. router-access     (no dependencies)
3. router-middleware (no dependencies)
4. router-timelock   (no dependencies)
5. router-multicall  (no dependencies)
6. router-core       (logically depends on the others, deploy last)
```

---

## Step-by-Step Deployment

Replace `<NETWORK>` with `testnet` or `mainnet` and `<ACCOUNT>` with your key name.

### 1. Deploy router-registry

```bash
REGISTRY_ID=$(stellar contract deploy \
  --wasm target/wasm32-unknown-unknown/release/router_registry.wasm \
  --network <NETWORK> --source <ACCOUNT>)
echo "registry: $REGISTRY_ID"

stellar contract invoke --id $REGISTRY_ID --network <NETWORK> --source <ACCOUNT> \
  -- initialize --admin <ADMIN_ADDRESS>
```

### 2. Deploy router-access

```bash
ACCESS_ID=$(stellar contract deploy \
  --wasm target/wasm32-unknown-unknown/release/router_access.wasm \
  --network <NETWORK> --source <ACCOUNT>)
echo "access: $ACCESS_ID"

stellar contract invoke --id $ACCESS_ID --network <NETWORK> --source <ACCOUNT> \
  -- initialize --super_admin <ADMIN_ADDRESS>
```

### 3. Deploy router-middleware

```bash
MIDDLEWARE_ID=$(stellar contract deploy \
  --wasm target/wasm32-unknown-unknown/release/router_middleware.wasm \
  --network <NETWORK> --source <ACCOUNT>)
echo "middleware: $MIDDLEWARE_ID"

stellar contract invoke --id $MIDDLEWARE_ID --network <NETWORK> --source <ACCOUNT> \
  -- initialize --admin <ADMIN_ADDRESS>
```

### 4. Deploy router-timelock

```bash
TIMELOCK_ID=$(stellar contract deploy \
  --wasm target/wasm32-unknown-unknown/release/router_timelock.wasm \
  --network <NETWORK> --source <ACCOUNT>)
echo "timelock: $TIMELOCK_ID"

# min_delay in seconds — use 86400 (24h) for mainnet, 3600 (1h) for testnet
stellar contract invoke --id $TIMELOCK_ID --network <NETWORK> --source <ACCOUNT> \
  -- initialize --admin <ADMIN_ADDRESS> --min_delay 86400
```

### 5. Deploy router-multicall

```bash
MULTICALL_ID=$(stellar contract deploy \
  --wasm target/wasm32-unknown-unknown/release/router_multicall.wasm \
  --network <NETWORK> --source <ACCOUNT>)
echo "multicall: $MULTICALL_ID"

stellar contract invoke --id $MULTICALL_ID --network <NETWORK> --source <ACCOUNT> \
  -- initialize --admin <ADMIN_ADDRESS> --max_batch_size 10
```

### 6. Deploy router-core

```bash
CORE_ID=$(stellar contract deploy \
  --wasm target/wasm32-unknown-unknown/release/router_core.wasm \
  --network <NETWORK> --source <ACCOUNT>)
echo "core: $CORE_ID"

stellar contract invoke --id $CORE_ID --network <NETWORK> --source <ACCOUNT> \
  -- initialize --admin <ADMIN_ADDRESS>
```

---

## Post-Deployment Verification

```bash
# Verify router-core is initialized
stellar contract invoke --id $CORE_ID --network <NETWORK> --source <ACCOUNT> \
  -- admin

# Register a test route
stellar contract invoke --id $CORE_ID --network <NETWORK> --source <ACCOUNT> \
  -- register_route \
  --caller <ADMIN_ADDRESS> --name test --address $REGISTRY_ID

# Resolve it
stellar contract invoke --id $CORE_ID --network <NETWORK> --source <ACCOUNT> \
  -- resolve --name test
```

---

## Environment Variables Reference

For the metrics exporter and api-server:

| Variable | Default | Description |
|---|---|---|
| `SOROBAN_RPC_URL` | `https://soroban-testnet.stellar.org` | Soroban RPC endpoint |
| `ROUTER_CORE_CONTRACT_ID` | — | Deployed router-core contract ID |
| `ROUTER_REGISTRY_CONTRACT_ID` | — | Deployed router-registry contract ID |
| `ROUTER_ACCESS_CONTRACT_ID` | — | Deployed router-access contract ID |
| `ROUTER_MIDDLEWARE_CONTRACT_ID` | — | Deployed router-middleware contract ID |
| `ROUTER_TIMELOCK_CONTRACT_ID` | — | Deployed router-timelock contract ID |
| `ROUTER_MULTICALL_CONTRACT_ID` | — | Deployed router-multicall contract ID |
| `ROUTER_AUTH_ENABLED` | `false` | Enable API key auth on the api-server |
| `ROUTER_API_KEY` | — | API key (required if auth enabled) |
| `ROUTER_REPLAY_PROTECTION_ENABLED` | `false` | Enable nonce-based replay protection |
| `LISTEN_ADDR` | `127.0.0.1:8080` | api-server listen address |
| `RUST_LOG` | `info` | Log level |

---

## Docker Compose (Local Development)

```bash
# Start metrics exporter + Prometheus + Grafana
docker compose up

# Run tests only
docker compose run tests

# Build WASM artifacts
docker compose run wasm
```

Prometheus: http://localhost:9091  
Grafana: http://localhost:3000

---

## Performance Tuning Guide

This section provides guidance on tuning key performance parameters for optimal operation in production.

### router-multicall: Optimal max_batch_size

The `max_batch_size` parameter controls the maximum number of cross-contract calls that can be executed in a single transaction.

**Default:** 10  
**Range:** 1–100 (practical upper bound)

**Trade-offs:**
- **Smaller batches (5–10):** Lower gas cost per transaction, faster execution, less risk of timeout
- **Larger batches (20–50):** Fewer transactions, better for bulk operations, higher gas cost per transaction
- **Very large batches (50+):** Risk of hitting Soroban's transaction size limits, increased failure probability

**Recommendations:**
- **Read-heavy workloads:** Use 20–30 for aggregating data from multiple contracts
- **Write-heavy workloads:** Use 5–10 to minimize gas costs and failure risk
- **Mixed workloads:** Start with 10 and monitor failure rates; increase if failures are low

**Configuration:**
```bash
stellar contract invoke --id $MULTICALL_ID --network <NETWORK> --source <ACCOUNT> \
  -- set_max_batch_size --caller <ADMIN_ADDRESS> --new_max 20
```

**Monitoring:**
- Track `batch_executed` events for success/failure rates
- Monitor `budget_exceeded_count` in `BatchSummary` for CPU/memory issues
- If failure rate > 5%, reduce `max_batch_size`

---

### router-middleware: Rate Limit Window Sizing

Rate limiting uses `max_calls_per_window` and `window_seconds` to control request velocity per `(route, caller)` pair.

**Formula:** `calls_per_second = max_calls_per_window / window_seconds`

**Common Configurations:**

| Use Case | max_calls_per_window | window_seconds | Rate | Rationale |
|---|---|---|---|---|
| Public APIs | 100 | 3600 | 0.028/sec | Prevent abuse, allow burst |
| Internal services | 1000 | 60 | 16.7/sec | Higher throughput for trusted callers |
| High-frequency trading | 500 | 10 | 50/sec | Sub-second latency requirements |
| Rate-limited endpoints | 10 | 60 | 0.17/sec | Strict throttling |

**Tuning Guidelines:**
1. **Start conservative:** Begin with 100 calls per 3600 seconds (1 hour window)
2. **Measure actual load:** Use metrics exporter to track `total_calls` and `rate_limit_state`
3. **Adjust based on patterns:**
   - If callers hit limits frequently: increase `max_calls_per_window` or decrease `window_seconds`
   - If abuse detected: decrease limits or add role-based access control
4. **Window size considerations:**
   - **Short windows (10–60s):** Better for detecting bursts, more storage churn
   - **Long windows (3600s+):** Better for sustained rate limiting, less storage overhead

**Configuration:**
```bash
stellar contract invoke --id $MIDDLEWARE_ID --network <NETWORK> --source <ACCOUNT> \
  -- configure_route \
  --caller <ADMIN_ADDRESS> --route oracle/get_price \
  --max_calls_per_window 1000 --window_seconds 60 \
  --enabled true --failure_threshold 5 --recovery_window_seconds 300
```

**Set to 0 to disable rate limiting:**
```bash
--max_calls_per_window 0
```

---

### router-middleware: Circuit Breaker Threshold Tuning

The circuit breaker uses `failure_threshold` and `recovery_window_seconds` to prevent cascading failures.

**Parameters:**
- `failure_threshold`: Number of consecutive failures before tripping (0 = disabled)
- `recovery_window_seconds`: Minimum time before circuit can auto-reset (in seconds)

**Recommended Configurations:**

| Scenario | failure_threshold | recovery_window_seconds | Use Case |
|---|---|---|---|
| Conservative | 3 | 900 (15 min) | Critical infrastructure, low tolerance for downtime |
| Balanced | 5 | 300 (5 min) | General production use |
| Aggressive | 10 | 60 (1 min) | High-availability services with fast recovery |
| Disabled | 0 | — | Development/testing only |

**Tuning Guidelines:**
1. **Start with balanced settings:** `failure_threshold = 5`, `recovery_window_seconds = 300`
2. **Monitor failure patterns:**
   - If circuit trips too frequently (false positives): increase `failure_threshold`
   - If circuit doesn't trip when it should (false negatives): decrease `failure_threshold`
3. **Recovery window sizing:**
   - **Too short (< 60s):** Vulnerable to griefing attacks where attacker repeatedly trips circuit
   - **Too long (> 1800s):** Excessive downtime for transient failures
   - **Sweet spot:** 300–900 seconds (5–15 minutes)
4. **Manual reset:** Use `reset_circuit_breaker` (admin-only) to manually clear a tripped circuit if recovery is faster than expected

**Configuration:**
```bash
stellar contract invoke --id $MIDDLEWARE_ID --network <NETWORK> --source <ACCOUNT> \
  -- configure_route \
  --caller <ADMIN_ADDRESS> --route oracle/get_price \
  --max_calls_per_window 100 --window_seconds 3600 \
  --enabled true --failure_threshold 5 --recovery_window_seconds 300
```

**Monitoring:**
- Watch `circuit_opened` events for trip frequency
- Track `post_call` events to identify failure patterns
- Alert on repeated trips within short time windows

---

### Gas Cost Estimates per Operation

Soroban gas costs are measured in stroops (1 stroop = 0.0000001 XLM). Costs vary by network conditions and operation complexity.

**Estimated Costs (Testnet/Mainnet):**

| Operation | Base Fee (stroops) | Resource Fee (stroops) | Total (stroops) | Total (XLM) |
|---|---|---|---|---|
| router-core resolve | 100 | 5,000–15,000 | 5,100–15,100 | 0.00051–0.00151 |
| router-core register_route | 100 | 20,000–50,000 | 20,100–50,100 | 0.00201–0.00501 |
| router-multicall execute_batch (5 calls) | 100 | 25,000–75,000 | 25,100–75,100 | 0.00251–0.00751 |
| router-multicall execute_batch (10 calls) | 100 | 50,000–150,000 | 50,100–150,100 | 0.00501–0.01501 |
| router-middleware pre_call | 100 | 3,000–8,000 | 3,100–8,100 | 0.00031–0.00081 |
| router-timelock queue | 100 | 15,000–40,000 | 15,100–40,100 | 0.00151–0.00401 |
| router-timelock execute | 100 | 20,000–60,000 | 20,100–60,100 | 0.00201–0.00601 |
| router-access grant_role | 100 | 10,000–30,000 | 10,100–30,100 | 0.00101–0.00301 |

**Factors Affecting Gas Costs:**
- **Network congestion:** Higher during peak usage
- **Contract complexity:** More storage reads/writes = higher cost
- **Batch size:** Linear scaling with number of operations
- **Instruction budget:** CPU-intensive operations cost more

**Cost Optimization Strategies:**
1. **Batch operations:** Use router-multicall to combine multiple calls
2. **Minimize storage writes:** Cache data off-chain when possible
3. **Use simulation:** Run `simulate = true` in router-multicall to test without gas cost
4. **Monitor network conditions:** Adjust operations based on current fee rates

**Fee Estimation:**
Use router-execution's `estimate_fee` function for real-time estimates:
```bash
stellar contract invoke --id $EXECUTION_ID --network <NETWORK> --source <ACCOUNT> \
  -- estimate_fee --operation <OPERATION_XDR>
```

---

### General Performance Best Practices

1. **Start conservative, iterate:**
   - Begin with default values
   - Monitor metrics for 24–48 hours
   - Adjust based on observed patterns

2. **Use testnet for tuning:**
   - Test configuration changes on testnet first
   - Simulate production load patterns
   - Measure gas costs and failure rates

3. **Monitor continuously:**
   - Set up metrics exporter with Prometheus
   - Configure Grafana dashboards for key metrics
   - Set alerts on abnormal patterns (high failure rates, circuit trips)

4. **Document changes:**
   - Track configuration changes in a changelog
   - Record rationale for tuning decisions
   - Maintain rollback plans for problematic changes

5. **Plan for scaling:**
   - Design configuration for expected peak load
   - Have procedures for emergency scaling (e.g., increasing rate limits during events)
   - Consider automated scaling based on metrics

---

## Mainnet Checklist

Before deploying to mainnet:

- [ ] All contracts tested on testnet with production-like data
- [ ] Admin keypair is a hardware wallet or multi-sig account
- [ ] `min_delay` in router-timelock set to at least 24 hours (86400)
- [ ] All contract IDs recorded and backed up
- [ ] Monitoring set up (metrics exporter + alerting rules)
- [ ] `initialize()` called on every contract before registering routes
- [ ] Test route registered and resolved successfully

---

## Troubleshooting

**`Error: contract not found`**  
The contract ID is wrong or the contract was not deployed to this network. Verify with `stellar contract inspect --id <ID> --network <NETWORK>`.

**`Error: not initialized`**  
`initialize()` was not called after deployment. Call it before any other function.

**`Error: unauthorized`**  
The `--source` account does not match the admin address set during `initialize()`. Use the same account that initialized the contract.

**`Error: insufficient funds`**  
The source account does not have enough XLM to pay transaction fees. Fund it via Friendbot (testnet) or transfer XLM (mainnet).

**`Error: simulation failed`**  
The transaction would fail on-chain. Check that all arguments are correct and the contract is initialized. Run with `--verbose` for more detail.

**Contract ID starts with `G` instead of `C`**  
You are using an account address instead of a contract ID. Contract IDs always start with `C`.

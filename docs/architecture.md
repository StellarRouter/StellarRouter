# Architecture

## Overview

`stellar-router` is a modular suite of Soroban smart contracts for building composable,
upgradeable, and access-controlled multi-contract systems on Stellar. Each contract is
independently deployable and has no hard dependency on the others.

```
┌─────────────────────────────────────────────────────┐
│                    router-core                      │
│         Central dispatcher & route resolver         │
└────────────┬────────────────────────┬───────────────┘
             │                        │
    ┌────────▼────────┐      ┌────────▼────────┐
    │ router-registry │      │  router-access  │
    │ Versioned addr  │      │  Role-based ACL │
    │ book            │      │  & blacklisting │
    └─────────────────┘      └─────────────────┘
             │                        │
    ┌────────▼────────┐      ┌────────▼────────┐
    │router-middleware│      │router-timelock  │
    │ Rate limiting   │      │ Delayed change  │
    │ Call logging    │      │ execution queue │
    └─────────────────┘      └─────────────────┘
                      │
             ┌────────▼────────┐
             │router-multicall │
             │ Batch calls in  │
             │ one transaction │
             └─────────────────┘
```

Off-chain components connect to the on-chain contracts via the Soroban RPC endpoint:

```
[Stellar Network]
      │
      │  Soroban RPC
      │
[router-metrics-exporter]  ──►  Prometheus  ──►  Grafana
[api-server]               ──►  REST / WebSocket clients
```

---

## Contracts

### router-core

The entry point for all routing. Maintains a `name → address` mapping and resolves
contract addresses by route name. Supports pause controls at both the global and
per-route level.

**Key functions:**
- `initialize(admin)` — set the admin and activate the contract.
- `register_route(caller, name, address)` — map a name to a contract address.
- `resolve(name) → Address` — look up the address for a route name.
- `pause_route(caller, name)` / `unpause_route(caller, name)` — toggle a route.
- `pause_all(caller)` / `unpause_all(caller)` — global pause toggle.

**Events emitted:** `route_registered`, `route_resolved`, `route_paused`, `route_unpaused`.

---

### router-registry

A versioned address book. Each entry is keyed by `(name, version)`. Versions must
increase monotonically. Old versions can be deprecated with an optional reason string,
and `get_latest` always returns the newest non-deprecated entry.

**Key functions:**
- `register(caller, name, address, version)` — add a new versioned entry.
- `get(name, version) → ContractEntry` — fetch a specific version.
- `get_latest(name) → ContractEntry` — fetch the newest non-deprecated version.
- `get_latest_with_constraint(name, constraint)` — fetch with a semver-style constraint (e.g. `>=2`, `<3`).
- `deprecate(caller, name, version, reason)` — mark a version deprecated with an optional reason.
- `deprecate_many(caller, entries)` — batch deprecation.

**ContractEntry fields:** `address`, `name`, `version`, `deprecated`, `deprecation_reason`, `registered_by`.

**Events emitted:** `contract_registered(name, version, None)`, `contract_deprecated(name, version, reason)`, `admin_transferred`.

---

### router-access

Role-based access control with three tiers:

| Tier | Capability |
|---|---|
| Super admin | Full control — grant/revoke any role, blacklist addresses |
| Role admin | Grant/revoke a specific named role |
| Role member | Holds a named role |

Addresses can be blacklisted to prevent them from being granted any role.

**Key functions:**
- `grant_role(caller, role, address)` / `revoke_role(caller, role, address)`
- `has_role(role, address) → bool`
- `blacklist(caller, address)` / `unblacklist(caller, address)`

---

### router-middleware

Pre/post call hooks for any route. Supports per-route rate limiting, global and
per-route enable/disable toggles, and call event logging.

**Key functions:**
- `configure_route(caller, route, max_calls_per_window, window_seconds, enabled)`
- `pre_call(caller, route)` — check rate limit and log the call attempt.
- `post_call(caller, route, success)` — log the call result.
- `enable_route(caller, route)` / `disable_route(caller, route)`

**Events emitted:** `pre_call_logged`, `post_call_logged`, `route_configured`.

---

### router-timelock

A delay queue for sensitive router changes (e.g. upgrading a registry entry).
Operations must wait a configurable minimum delay before they can be executed.
Operations can be cancelled before execution.

**Key functions:**
- `queue(proposer, description, target, delay) → op_id` — enqueue an operation.
- `execute(caller, op_id)` — execute after the ETA has passed.
- `cancel(caller, op_id)` — cancel before execution.

**States:** `Pending → Ready → Executed | Cancelled`.

---

### router-multicall

Batches multiple cross-contract calls into a single transaction. Each call can be
marked `required` (failure aborts the batch) or optional (failure is tracked but
does not abort). Returns a `BatchSummary` with success/failure counts.

`execute_batch` is a public function — any authenticated address can call it.
The admin role is only used for configuration (e.g. `set_max_batch_size`).

---

### router-execution

Execution pipeline with simulation, retries, and fee estimation. Wraps cross-contract
calls with retry logic and pre-execution simulation to estimate gas costs.

---

### router-quote

Read-only quote preview contract. Returns expected output amount, fees, exchange rate,
and price impact without executing the transaction.

**Key functions:**
- `get_quote(router_core, route_name, token_in, token_out, amount_in, fee_bps, slippage_bps, precision) → QuoteResponse`
- `get_multihop_quote(hops, amount_in, slippage_bps, precision) → QuoteResponse`
- `estimate_fee(request) → FeeEstimateResponse`

**QuoteResponse fields:** `amount_out`, `fee_amount`, `min_amount_out`, `exchange_rate`, `precision`, `price_impact_bps`.

`price_impact_bps` is calculated as `(amount_out - amount_in) * 10_000 / amount_in`.
Negative values indicate adverse price impact (user receives less than they put in).

---

## Data Flow: Typical Route Resolution

```
Caller
  │
  │  resolve("oracle")
  ▼
router-core
  │  checks: not paused globally, route not paused
  │
  │  emits: route_resolved
  │
  └──► returns Address of oracle contract
         │
         │  Caller then invokes oracle directly
         ▼
       oracle contract
```

With middleware:

```
Caller
  │
  │  pre_call("oracle/get_price")
  ▼
router-middleware
  │  checks: route enabled, rate limit not exceeded
  │  emits: pre_call_logged
  │
  ▼
Caller invokes oracle
  │
  │  post_call("oracle/get_price", success=true)
  ▼
router-middleware
  │  emits: post_call_logged
```

With timelock (for sensitive config changes):

```
Admin
  │  queue("upgrade oracle to v2", target=new_addr, delay=86400)
  ▼
router-timelock  ──►  stores op with eta = now + delay
  │
  │  (24 hours pass)
  │
Admin
  │  execute(op_id)
  ▼
router-timelock  ──►  executes the queued operation
```

---

## Off-Chain Components

### router-metrics-exporter

An off-chain Rust binary that polls the Soroban RPC endpoint and exposes contract
metrics in Prometheus format. Grafana dashboards are provided for visualization.

```
router-metrics-exporter
  │  polls Soroban RPC every N seconds
  │  reads ledger entries and event streams
  │
  ├──► /metrics  (Prometheus scrape endpoint)
  │
Prometheus  ──►  Grafana (dashboards + alerts)
```

Configuration via environment variables:
- `ROUTER_RPC_URL` — Soroban RPC endpoint URL.
- `ROUTER_CORE_CONTRACT_ID` — deployed router-core contract ID.
- `ROUTER_MIDDLEWARE_CONTRACT_ID` — deployed router-middleware contract ID.
- `ROUTER_REGISTRY_CONTRACT_ID` — deployed router-registry contract ID.

See [`metrics/README.md`](../metrics/README.md) for full setup instructions.

### api-server

A REST and WebSocket API server that proxies Soroban RPC calls and provides a
higher-level interface for off-chain clients. Supports route resolution, quote
fetching, and event streaming.

---

## Deployment Order

Contracts have no hard on-chain dependencies, but the recommended deployment order
ensures that router-core is available before contracts that optionally integrate with it:

1. `router-registry`
2. `router-access`
3. `router-middleware`
4. `router-timelock`
5. `router-multicall`
6. `router-core`
7. `router-execution` (optionally references router-core)
8. `router-quote` (optionally references router-core for route resolution)

See [`docs/deployment.md`](deployment.md) for full deployment instructions.

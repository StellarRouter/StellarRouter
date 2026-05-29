# stellar-router [![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](LICENSE) [![Language: Rust](https://img.shields.io/badge/language-Rust-orange.svg)](https://www.rust-lang.org/)

A modular cross-contract routing infrastructure suite for Stellar/Soroban.

## Overview

`stellar-router` provides a complete set of infrastructure primitives for building
composable, upgradeable, and access-controlled multi-contract systems on Soroban.

```
┌─────────────────────────────────────────────────────┐
│                    router-core                      │
│         Central dispatcher & route resolver         │
└────────────┬────────────────────────┬───────────────┘
             │                        │
    ┌────────▼────────┐      ┌────────▼────────┐
    │ router-registry │      │  router-access  │
    │ Contract address│      │  Role-based ACL │
    │ versioning      │      │  & whitelisting │
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

## Contracts

| Contract | Description | Tests |
|---|---|---|
| `router-core` | Central dispatcher, route registration/resolution, pause controls | 8 |
| `router-registry` | Versioned contract address registry with deprecation support | 8 |
| `router-access` | Role-based access control, blacklisting, and role admins | 7 |
| `router-middleware` | Rate limiting, route enable/disable, and call event logging | 6 |
| `router-timelock` | Delayed execution queue for sensitive configuration changes | 7 |
| `router-multicall` | Batch multiple cross-contract calls in one transaction | 6 |
| `router-quote` | Configurable fee-based quote calculation and best-route selection | 13 |

## Architecture

### router-core
The entry point for all routing. Maintains a name → address mapping and resolves
contract addresses by route name. Supports pause controls at both the global and
per-route level. Emits events on every resolution.

### router-registry
A versioned address book. Each entry is keyed by `(name, version)`. Versions must
increase monotonically. Old versions can be deprecated, and `get_latest` always
returns the newest non-deprecated entry.

### router-access
Role-based access control with three tiers:
- **Super admin** — can do everything
- **Role admin** — can grant/revoke a specific named role
- **Role members** — hold a named role

Addresses can be blacklisted to prevent them from being granted any role.

### router-middleware
Pre/post call hooks for any route. Supports:
- Per-route rate limiting (max calls per time window)
- Global enable/disable toggle
- Per-route enable/disable
- Call event logging via `pre_call` / `post_call`

### router-timelock
A delay queue for sensitive router changes (e.g. upgrading a registry entry).
Operations must wait a configurable minimum delay before they can be executed.
Operations can be cancelled before execution.

### router-multicall
Batches multiple cross-contract calls into a single transaction. Each call can be
marked `required` (failure aborts the batch) or optional (failure is tracked but
does not abort). Returns a `BatchSummary` with success/failure counts.

**Access Model:** `execute_batch` is a public function — any authenticated address
can call it, not just the admin. This is intentional: `router-multicall` is designed
as a public batching service where any caller can batch their own cross-contract
calls to reduce round-trips. The admin role is only used for configuration (e.g.,
setting `max_batch_size`).

### router-quote
Quote calculation and route comparison. Provides configurable fee-based quote
calculations and best-route selection for comparing multiple liquidity routes.

Key features:
- **Configurable fee_bps per route** — each route can have its own fee in basis
  points (1 bps = 0.01%). Falls back to a configurable default fee if no
  route-specific fee is set. Replaces the old hardcoded 1% fee.
- **`get_quote(request)`** — calculates a single quote with the route's configured
  `fee_bps`, returning `amount_out`, `fee_amount`, and `fee_bps` used.
- **`get_quotes(requests)`** — calculates quotes for multiple routes at once.
- **`get_best_quote(requests)`** — calls `get_quotes()` internally and returns the
  single `QuoteResponse` with the highest `amount_out`. Useful for automatic
  route comparison and selection.

## Getting Started

### Prerequisites
- Rust (stable)
- Soroban CLI: `cargo install --locked stellar-cli`

### Build

```bash
git clone https://github.com/Maki-Zeninn/stellar-router.git
cd stellar-router
cargo build
```

### Test

```bash
cargo test
```

### Build for Deployment (WASM)

```bash
cargo build --target wasm32-unknown-unknown --release
```

WASM files will be output to:
```
target/wasm32-unknown-unknown/release/router_core.wasm
target/wasm32-unknown-unknown/release/router_registry.wasm
target/wasm32-unknown-unknown/release/router_access.wasm
target/wasm32-unknown-unknown/release/router_middleware.wasm
target/wasm32-unknown-unknown/release/router_timelock.wasm
target/wasm32-unknown-unknown/release/router_multicall.wasm
```

## Deployment

Deploy contracts to testnet in dependency order:

```bash
# 1. Deploy registry
stellar contract deploy \
  --wasm target/wasm32-unknown-unknown/release/router_registry.wasm \
  --network testnet --source <your-account>

# 2. Deploy access
stellar contract deploy \
  --wasm target/wasm32-unknown-unknown/release/router_access.wasm \
  --network testnet --source <your-account>

# 3. Deploy middleware
stellar contract deploy \
  --wasm target/wasm32-unknown-unknown/release/router_middleware.wasm \
  --network testnet --source <your-account>

# 4. Deploy timelock
stellar contract deploy \
  --wasm target/wasm32-unknown-unknown/release/router_timelock.wasm \
  --network testnet --source <your-account>

# 5. Deploy multicall
stellar contract deploy \
  --wasm target/wasm32-unknown-unknown/release/router_multicall.wasm \
  --network testnet --source <your-account>

# 6. Deploy core last (depends on all others)
stellar contract deploy \
  --wasm target/wasm32-unknown-unknown/release/router_core.wasm \
  --network testnet --source <your-account>
```

## Example Usage

### Register a contract in the router

```bash
# Initialize router-core
stellar contract invoke --id <CORE_ID> --network testnet --source admin \
  -- initialize --admin <ADMIN_ADDRESS>

# Register an oracle contract as a route
stellar contract invoke --id <CORE_ID> --network testnet --source admin \
  -- register_route \
  --caller <ADMIN_ADDRESS> \
  --name oracle \
  --address <ORACLE_CONTRACT_ID>

# Resolve the oracle address
stellar contract invoke --id <CORE_ID> --network testnet --source admin \
  -- resolve --name oracle
```

### Set up rate limiting via middleware

```bash
stellar contract invoke --id <MIDDLEWARE_ID> --network testnet --source admin \
  -- configure_route \
  --caller <ADMIN_ADDRESS> \
  --route "oracle/get_price" \
  --max_calls_per_window 100 \
  --window_seconds 3600 \
  --enabled true
```

### Queue a timelock operation

```bash
stellar contract invoke --id <TIMELOCK_ID> --network testnet --source admin \
  -- queue \
  --proposer <ADMIN_ADDRESS> \
  --description "upgrade oracle to v2" \
  --target <NEW_ORACLE_ADDRESS> \
  --delay 86400
```

## FAQ

**What is Soroban?**
Soroban is the smart contract platform built into the Stellar network, designed for
predictable performance and low fees. See the [official docs](https://developers.stellar.org/docs/build/smart-contracts/overview) for more.

**Do I need to deploy all 6 contracts?**
No. `router-core` is the only required contract — it handles route registration and
resolution. The others are optional enhancements:
- `router-registry` — only needed if you want versioned contract address management
- `router-access` — only needed if you want role-based access control
- `router-middleware` — only needed if you want rate limiting or call hooks
- `router-timelock` — only needed if you want delayed execution of config changes
- `router-multicall` — only needed if you want to batch multiple calls in one transaction

**Can I use just one contract from this suite?**
Yes. Each contract is independently deployable and usable. They are designed to
complement each other but have no hard dependencies between them. You can deploy
only the contracts that fit your use case.

**What network should I use for development?**
Use the Stellar **testnet**. It is a public network that mirrors mainnet behaviour
but uses test tokens with no real value, so you can deploy and iterate freely without
any cost. You can fund a testnet account using the
[Stellar Friendbot](https://developers.stellar.org/docs/learn/fundamentals/networks).
Only move to **mainnet** when your contracts are fully tested and audited.

## License

MIT

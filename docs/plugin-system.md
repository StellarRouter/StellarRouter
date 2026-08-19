# Plugin System for Liquidity Sources

A modular interface for registering and querying liquidity providers via router-registry.

## Concept

Each liquidity provider is a separate Soroban contract that implements the
LiquidityPlugin interface. Plugins are registered in router-registry under a
well-known name (e.g. `"liquidity/example-fixed-rate"`) and resolved via router-core.

## Plugin Interface

Every plugin contract must expose:

```rust
fn get_quote(env: Env, token_in: Address, token_out: Address, amount_in: i128) -> Result<i128, PluginError>
fn execute_swap(env: Env, caller: Address, token_in: Address, token_out: Address,
                amount_in: i128, min_amount_out: i128) -> Result<i128, PluginError>
fn name(env: Env) -> String
fn version(env: Env) -> u32
```

> **Note:** `execute_swap` should call `caller.require_auth()` before acting, and
> must reject trades whose output falls below `min_amount_out` (slippage guard).
> The reference implementation returns `Result<i128, _>` so invalid inputs
> (`token_in == token_out`, non-positive `amount_in`, insufficient output) can be
> surfaced as contract errors instead of panics.

## Registering a Plugin

```bash
stellar contract invoke --id <REGISTRY_ID> \
  -- register \
  --caller <ADMIN> \
  --name "liquidity/example-fixed-rate" \
  --address <PLUGIN_CONTRACT_ID> \
  --version 1
```

## Routing to a Plugin

```bash
stellar contract invoke --id <CORE_ID> \
  -- register_route \
  --caller <ADMIN> \
  --name "liquidity/example-fixed-rate" \
  --address <PLUGIN_CONTRACT_ID>
```

## Reference Implementation

A complete, tested reference plugin lives in
[`contracts/liquidity-plugin-example-fixed-rate`](../contracts/liquidity-plugin-example-fixed-rate).
It is a deliberately minimal fixed-rate mock DEX (default 30 bps fee, admin-adjustable)
that implements the full interface without depending on external token contracts.

The in-process integration test
[`integration-tests/tests/liquidity_plugin_tests.rs`](../integration-tests/tests/liquidity_plugin_tests.rs)
walks the full lifecycle: register the plugin in router-registry under its
well-known name, point a router-core route at it, resolve the route back to the
plugin address, then quote and execute a swap through that address.

```rust
#![no_std]

use soroban_sdk::{contract, contracterror, contractimpl, Address, Env, String};

const DEFAULT_FEE_BPS: i128 = 30;

#[contracterror]
#[derive(Copy, Clone, Debug, PartialEq)]
pub enum PluginError {
    NotInitialized = 2,
    Unauthorized = 3,
    SameToken = 4,
    InvalidAmount = 5,
    InsufficientOutput = 7,
}

#[contract]
pub struct MyDexPlugin;

#[contractimpl]
impl MyDexPlugin {
    pub fn get_quote(
        env: Env,
        token_in: Address,
        token_out: Address,
        amount_in: i128,
    ) -> Result<i128, PluginError> {
        if token_in == token_out {
            return Err(PluginError::SameToken);
        }
        if amount_in <= 0 {
            return Err(PluginError::InvalidAmount);
        }
        Ok(amount_in * (10_000 - DEFAULT_FEE_BPS) / 10_000) // 0.3% fee
    }

    pub fn execute_swap(
        env: Env,
        caller: Address,
        token_in: Address,
        token_out: Address,
        amount_in: i128,
        min_amount_out: i128,
    ) -> Result<i128, PluginError> {
        caller.require_auth();
        let amount_out = Self::get_quote(env, token_in, token_out, amount_in)?;
        if amount_out < min_amount_out {
            return Err(PluginError::InsufficientOutput);
        }
        Ok(amount_out)
    }

    pub fn name(_env: Env) -> String {
        String::from_str(&_env, "liquidity/my-dex")
    }

    pub fn version(_env: Env) -> u32 {
        1
    }
}
```

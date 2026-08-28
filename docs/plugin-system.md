# Plugin System for Liquidity Sources

A modular interface for registering and querying liquidity providers via router-registry.

## Concept

Each liquidity provider is a separate Soroban contract that implements the
LiquidityPlugin interface. Plugins are registered in router-registry under a
well-known name (e.g. `"liquidity/example-fixed-rate"`) and resolved via router-core.

## Plugin Interface

The interface is defined as a Soroban contract trait in
[`contracts/router-plugin-interface`](../contracts/router-plugin-interface).
That crate is the canonical source — the signatures below mirror it:

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

### Errors

`PluginError` is part of the interface. Its discriminants are the numeric codes
callers observe on-chain, so they are stable across implementations and must not
be renumbered:

| Variant | Code | Meaning |
|---|---|---|
| `NotInitialized` | 2 | The plugin requires initialization before serving the call |
| `Unauthorized` | 3 | The caller may not perform this operation |
| `SameToken` | 4 | `token_in` equals `token_out` |
| `InvalidAmount` | 5 | `amount_in` is not positive |
| `InsufficientOutput` | 7 | Output fell below `min_amount_out` |

Failures that are not part of the plugin contract — initialization, fee
configuration, and similar — belong in the plugin's own error enum.

## Compile-Time Verification

Implement the trait and the compiler checks your contract against the interface.
A missing method, a reordered parameter, or a changed return type is a build
error rather than a surprise on-chain.

Add the dependency:

```toml
[dependencies]
router-plugin-interface = { path = "../router-plugin-interface" }
```

Then implement `LiquidityPlugin` inside a `#[contractimpl]` block:

```rust
#![no_std]

use router_plugin_interface::{LiquidityPlugin, PluginError};
use soroban_sdk::{contract, contractimpl, Address, Env, String};

#[contract]
pub struct MyDexPlugin;

#[contractimpl]
impl LiquidityPlugin for MyDexPlugin {
    fn get_quote(
        env: Env,
        token_in: Address,
        token_out: Address,
        amount_in: i128,
    ) -> Result<i128, PluginError> {
        // ...
    }

    fn execute_swap(
        env: Env,
        caller: Address,
        token_in: Address,
        token_out: Address,
        amount_in: i128,
        min_amount_out: i128,
    ) -> Result<i128, PluginError> {
        caller.require_auth();
        // ...
    }

    fn name(env: Env) -> String {
        String::from_str(&env, "liquidity/my-dex")
    }

    fn version(_env: Env) -> u32 {
        1
    }
}
```

Verify with:

```bash
cargo check -p my-dex-plugin --target wasm32-unknown-unknown
```

Methods your plugin needs beyond the interface (admin controls, configuration)
go in a second, plain `#[contractimpl] impl MyDexPlugin` block.

### Calling a plugin

`LiquidityPluginClient` is generated from the trait and invokes any contract
that satisfies the interface, so a caller needs the plugin's address and
nothing else:

```rust
use router_plugin_interface::LiquidityPluginClient;

let plugin_address = core.resolve(&route_name);
let plugin = LiquidityPluginClient::new(&env, &plugin_address);
let amount_out = plugin.get_quote(&token_in, &token_out, &amount_in);
```

Use the generated `try_*` methods (`try_get_quote`, `try_execute_swap`) to
handle `PluginError` instead of panicking.

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

## Reference Implementations

Two reference plugins ship with the repository:

- **`NoopLiquidityPlugin`**, in
  [`contracts/router-plugin-interface`](../contracts/router-plugin-interface) —
  the smallest contract that satisfies the interface. It quotes one-for-one with
  no fee and no state, so it doubles as a stand-in when testing routing without a
  real liquidity source.
- **[`contracts/liquidity-plugin-example-fixed-rate`](../contracts/liquidity-plugin-example-fixed-rate)** —
  a deliberately minimal fixed-rate mock DEX (default 30 bps fee,
  admin-adjustable) that implements the full interface without depending on
  external token contracts.

Two in-process integration test suites cover them:

- [`integration-tests/tests/liquidity_plugin_tests.rs`](../integration-tests/tests/liquidity_plugin_tests.rs)
  walks the full lifecycle: register the plugin in router-registry under its
  well-known name, point a router-core route at it, resolve the route back to the
  plugin address, then quote and execute a swap through that address.
- [`integration-tests/tests/plugin_interface_tests.rs`](../integration-tests/tests/plugin_interface_tests.rs)
  does the same through `LiquidityPluginClient`, which knows nothing about the
  contract behind the address — including a case that drives the fixed-rate
  example, confirming the trait describes the ABI the repository already ships.

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

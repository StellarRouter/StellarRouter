#![no_std]

//! # router-plugin-interface
//!
//! The canonical [`LiquidityPlugin`] interface for stellar-router liquidity
//! sources, expressed as a Soroban contract trait.
//!
//! [`docs/plugin-system.md`] describes the plugin system in prose. This crate
//! turns that prose into an enforceable contract: a liquidity plugin that
//! writes `impl LiquidityPlugin for MyPlugin` either matches the interface
//! exactly or fails to compile, so third-party integrators can verify
//! compatibility at build time instead of discovering a mismatch on-chain.
//!
//! [`docs/plugin-system.md`]: https://github.com/StellarRouter/StellarRouter/blob/main/docs/plugin-system.md
//!
//! ## Implementing the interface
//!
//! Depend on this crate, then implement the trait inside a `#[contractimpl]`
//! block. The compiler checks every signature against the interface:
//!
//! ```ignore
//! use router_plugin_interface::{LiquidityPlugin, PluginError};
//! use soroban_sdk::{contract, contractimpl, Address, Env, String};
//!
//! #[contract]
//! pub struct MyDexPlugin;
//!
//! #[contractimpl]
//! impl LiquidityPlugin for MyDexPlugin {
//!     fn get_quote(
//!         env: Env,
//!         token_in: Address,
//!         token_out: Address,
//!         amount_in: i128,
//!     ) -> Result<i128, PluginError> {
//!         // ...
//!     }
//!     // execute_swap, name, version follow.
//! }
//! ```
//!
//! Omitting a method, renaming a parameter type, or changing a return type is
//! a compile error — that is the guarantee this crate exists to provide.
//!
//! ## Calling a plugin
//!
//! [`LiquidityPluginClient`] is generated from the trait by
//! [`contractclient`](soroban_sdk::contractclient). It invokes *any* contract
//! that satisfies the interface, so router-core can resolve a route to a
//! plugin address and call it without knowing the concrete implementation:
//!
//! ```ignore
//! let plugin_address = core.resolve(&route_name);
//! let plugin = LiquidityPluginClient::new(&env, &plugin_address);
//! let amount_out = plugin.get_quote(&token_in, &token_out, &amount_in);
//! ```
//!
//! ## Reference implementations
//!
//! - [`NoopLiquidityPlugin`] — the minimal pass-through plugin in this crate,
//!   included so the interface always ships with a compiling, tested
//!   implementation.
//! - `liquidity-plugin-example-fixed-rate` — a fuller example with a fee
//!   model, admin controls and events.

use soroban_sdk::{contract, contractclient, contracterror, contractimpl, Address, Env, String};

// ── Errors ────────────────────────────────────────────────────────────────────

/// Errors every [`LiquidityPlugin`] implementation reports through the
/// interface.
///
/// The discriminants are part of the interface: they are the numeric codes a
/// caller observes on-chain, so they must stay stable across implementations
/// and must not be renumbered. Implementation-specific failures that are not
/// part of the plugin contract (initialization, configuration, and similar)
/// belong in the plugin's own error enum.
#[contracterror]
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
#[repr(u32)]
pub enum PluginError {
    /// The plugin requires initialization before it can serve this call.
    NotInitialized = 2,
    /// The caller is not permitted to perform this operation.
    Unauthorized = 3,
    /// `token_in` and `token_out` must be different addresses.
    SameToken = 4,
    /// `amount_in` must be positive.
    InvalidAmount = 5,
    /// The computed `amount_out` is below the caller's `min_amount_out`.
    InsufficientOutput = 7,
}

// ── Interface ─────────────────────────────────────────────────────────────────

/// The interface every liquidity plugin must expose.
///
/// Implementations are separate Soroban contracts, registered in
/// router-registry under a well-known `"liquidity/<provider>"` name and
/// resolved through router-core. See the [crate docs](crate) for how to
/// implement and how to call the interface.
#[contractclient(name = "LiquidityPluginClient")]
pub trait LiquidityPlugin {
    /// Quote an input amount into an output amount for a token pair.
    ///
    /// Quoting must not modify state that changes the result of a subsequent
    /// [`execute_swap`](LiquidityPlugin::execute_swap) at the same ledger.
    ///
    /// # Arguments
    /// * `env` — The Soroban environment.
    /// * `token_in` — The address of the input token.
    /// * `token_out` — The address of the output token.
    /// * `amount_in` — The amount of input token; must be positive.
    ///
    /// # Returns
    /// The expected output amount, after any fees the plugin applies.
    ///
    /// # Errors
    /// * [`PluginError::SameToken`] — if `token_in` equals `token_out`.
    /// * [`PluginError::InvalidAmount`] — if `amount_in` is not positive.
    fn get_quote(
        env: Env,
        token_in: Address,
        token_out: Address,
        amount_in: i128,
    ) -> Result<i128, PluginError>;

    /// Execute a swap for the given input amount, enforcing slippage control.
    ///
    /// Implementations must call `caller.require_auth()` before acting, and
    /// must reject the trade when the output falls below `min_amount_out`.
    ///
    /// # Arguments
    /// * `env` — The Soroban environment.
    /// * `caller` — The address initiating the swap; must authenticate via
    ///   Soroban authorization.
    /// * `token_in` — The address of the input token.
    /// * `token_out` — The address of the output token.
    /// * `amount_in` — The amount of input token; must be positive.
    /// * `min_amount_out` — The minimum acceptable output amount.
    ///
    /// # Returns
    /// The actual output amount, which is always `>= min_amount_out`.
    ///
    /// # Errors
    /// * [`PluginError::SameToken`] — if `token_in` equals `token_out`.
    /// * [`PluginError::InvalidAmount`] — if `amount_in` is not positive.
    /// * [`PluginError::InsufficientOutput`] — if the computed output is below
    ///   `min_amount_out`.
    fn execute_swap(
        env: Env,
        caller: Address,
        token_in: Address,
        token_out: Address,
        amount_in: i128,
        min_amount_out: i128,
    ) -> Result<i128, PluginError>;

    /// Return the well-known name the plugin registers under in
    /// router-registry, following the `"liquidity/<provider>"` scheme.
    ///
    /// # Arguments
    /// * `env` — The Soroban environment.
    ///
    /// # Returns
    /// The plugin's registration name.
    fn name(env: Env) -> String;

    /// Return the plugin's version, reported alongside its name.
    ///
    /// # Arguments
    /// * `env` — The Soroban environment.
    ///
    /// # Returns
    /// The plugin version as a `u32`.
    fn version(env: Env) -> u32;
}

// ── Reference implementation ──────────────────────────────────────────────────

/// The well-known name [`NoopLiquidityPlugin`] registers under.
pub const NOOP_PLUGIN_NAME: &str = "liquidity/noop";

/// The version reported by [`NoopLiquidityPlugin`].
pub const NOOP_PLUGIN_VERSION: u32 = 1;

/// Minimal reference implementation of [`LiquidityPlugin`].
///
/// Quotes one-for-one — no fee, no spread, no state — so it does nothing
/// beyond satisfying the interface. It exists to keep a compiling, tested
/// implementation next to the trait, and it is a useful stand-in when testing
/// routing without a real liquidity source.
///
/// For a fuller example with fees, admin controls and events, see the
/// `liquidity-plugin-example-fixed-rate` contract.
#[contract]
pub struct NoopLiquidityPlugin;

#[contractimpl]
impl LiquidityPlugin for NoopLiquidityPlugin {
    /// Quote `amount_in` unchanged, after validating the pair and amount.
    fn get_quote(
        _env: Env,
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
        Ok(amount_in)
    }

    /// Return the quoted amount, rejecting outputs below `min_amount_out`.
    ///
    /// No tokens move: a real plugin would transfer `amount_in` in and
    /// `amount_out` out using the token contracts.
    fn execute_swap(
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

    /// Return [`NOOP_PLUGIN_NAME`].
    fn name(env: Env) -> String {
        String::from_str(&env, NOOP_PLUGIN_NAME)
    }

    /// Return [`NOOP_PLUGIN_VERSION`].
    fn version(_env: Env) -> u32 {
        NOOP_PLUGIN_VERSION
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    extern crate std;
    use super::*;
    use soroban_sdk::{testutils::Address as _, Env};

    fn setup() -> (Env, NoopLiquidityPluginClient<'static>) {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register_contract(None, NoopLiquidityPlugin);
        let client = NoopLiquidityPluginClient::new(&env, &contract_id);
        (env, client)
    }

    #[test]
    fn test_name_and_version() {
        let (env, client) = setup();
        assert_eq!(client.name(), String::from_str(&env, NOOP_PLUGIN_NAME));
        assert_eq!(client.version(), NOOP_PLUGIN_VERSION);
    }

    #[test]
    fn test_get_quote_is_one_for_one() {
        let (env, client) = setup();
        let token_in = Address::generate(&env);
        let token_out = Address::generate(&env);
        assert_eq!(client.get_quote(&token_in, &token_out, &1_000), 1_000);
    }

    #[test]
    fn test_get_quote_same_token_fails() {
        let (env, client) = setup();
        let token = Address::generate(&env);
        let result = client.try_get_quote(&token, &token, &1_000);
        assert_eq!(result, Err(Ok(PluginError::SameToken)));
    }

    #[test]
    fn test_get_quote_invalid_amount_fails() {
        let (env, client) = setup();
        let token_in = Address::generate(&env);
        let token_out = Address::generate(&env);
        let result = client.try_get_quote(&token_in, &token_out, &0);
        assert_eq!(result, Err(Ok(PluginError::InvalidAmount)));
    }

    #[test]
    fn test_execute_swap_success() {
        let (env, client) = setup();
        let caller = Address::generate(&env);
        let token_in = Address::generate(&env);
        let token_out = Address::generate(&env);
        assert_eq!(
            client.execute_swap(&caller, &token_in, &token_out, &1_000, &1_000),
            1_000
        );
    }

    #[test]
    fn test_execute_swap_min_amount_out_fails() {
        let (env, client) = setup();
        let caller = Address::generate(&env);
        let token_in = Address::generate(&env);
        let token_out = Address::generate(&env);
        let result = client.try_execute_swap(&caller, &token_in, &token_out, &1_000, &1_001);
        assert_eq!(result, Err(Ok(PluginError::InsufficientOutput)));
    }

    #[test]
    fn test_execute_swap_requires_auth() {
        let env = Env::default();
        let contract_id = env.register_contract(None, NoopLiquidityPlugin);
        let client = NoopLiquidityPluginClient::new(&env, &contract_id);
        let caller = Address::generate(&env);
        let token_in = Address::generate(&env);
        let token_out = Address::generate(&env);
        // With auth mocked OFF, an unauthenticated caller must be rejected.
        let result = client.try_execute_swap(&caller, &token_in, &token_out, &1_000, &0);
        assert!(result.is_err(), "unauthenticated execute_swap must fail");
    }

    /// The generated [`LiquidityPluginClient`] must be able to drive any
    /// contract that satisfies the interface — this is the call path
    /// router-core uses once it has resolved a route to a plugin address.
    #[test]
    fn test_generic_client_calls_any_implementation() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register_contract(None, NoopLiquidityPlugin);

        let plugin = LiquidityPluginClient::new(&env, &contract_id);
        let token_in = Address::generate(&env);
        let token_out = Address::generate(&env);

        assert_eq!(plugin.name(), String::from_str(&env, NOOP_PLUGIN_NAME));
        assert_eq!(plugin.version(), NOOP_PLUGIN_VERSION);
        assert_eq!(plugin.get_quote(&token_in, &token_out, &500), 500);
    }
}

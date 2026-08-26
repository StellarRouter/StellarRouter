#![no_std]

//! # liquidity-plugin-example-fixed-rate
//!
//! Reference implementation of the [`LiquidityPlugin`] interface described in
//! [`docs/plugin-system.md`]. It is a deliberately minimal, self-contained
//! example: a fixed-rate mock DEX that quotes and "executes" swaps entirely
//! inside the contract, without calling out to any token contracts.
//!
//! It exists so integrators have a complete, tested contract to reference when
//! building a real liquidity plugin.
//!
//! [`LiquidityPlugin`]: https://github.com/StellarRouter/StellarRouter/blob/main/docs/plugin-system.md
//! [`docs/plugin-system.md`]: https://github.com/StellarRouter/StellarRouter/blob/main/docs/plugin-system.md
//!
//! ## Interface
//!
//! The four entry points required by the plugin system are:
//! - [`get_quote`](Self::get_quote) — `amount_in` → `amount_out` after fee
//! - [`execute_swap`](Self::execute_swap) — simulated execution with slippage
//!   protection via `min_amount_out`
//! - [`name`](Self::name) — the well-known registration name
//! - [`version`](Self::version) — the plugin version
//!
//! ## Features
//! - Fixed fee model defaulting to 30 bps (0.30%), admin-adjustable
//! - Admin control (`initialize`, `set_fee`, `transfer_admin`) using the
//!   shared [`router-common`] macros
//! - Persistent swap counter to demonstrate on-contract state
//! - Events per the stellar-router naming convention (past-tense snake_case)
//!
//! ## Events
//! - `quote_generated` — A quote was computed (token_in, token_out, amount_in, amount_out, fee_bps)
//! - `swap_executed` — A swap was "executed" (caller, token_in, token_out, amount_in, amount_out)
//! - `fee_updated` — The fee was changed (admin, fee_bps)
//! - `admin_transferred` — Admin transferred (old_admin, new_admin)
//!
//! ## Registering (docs/plugin-system.md)
//!
//! 1. Deploy this contract.
//! 2. Register it in router-registry under its well-known name:
//!    `router-registry.register(admin, "liquidity/example-fixed-rate", <plugin>, 1)`.
//! 3. Point a router-core route at it:
//!    `router-core.register_route(admin, "liquidity/example-fixed-rate", <plugin>, None)`.
//! 4. `router-core.resolve("liquidity/example-fixed-rate")` returns the plugin
//!    address, which callers then use to `get_quote` / `execute_swap`.

use soroban_sdk::{
    contract, contracterror, contractimpl, contracttype, Address, Env, String, Symbol,
};

/// Default fee applied to every quote, in basis points (1 bps = 0.01%).
///
/// 30 bps = 0.30%. Used when the admin has not set a custom fee via
/// [`FixedRateLiquidityPlugin::set_fee`].
pub const DEFAULT_FEE_BPS: i128 = 30;

/// The well-known name this plugin registers under in router-registry.
///
/// Mirrors the `"liquidity/<provider>"` naming scheme from docs/plugin-system.md.
pub const PLUGIN_NAME: &str = "liquidity/example-fixed-rate";

/// Current plugin version, returned by [`FixedRateLiquidityPlugin::version`].
pub const PLUGIN_VERSION: u32 = 1;

// ── Storage Keys ──────────────────────────────────────────────────────────────

#[contracttype]
pub enum DataKey {
    Admin,
    /// Custom fee in basis points. Absent → [`DEFAULT_FEE_BPS`].
    FeeBps,
    /// Total number of executed swaps (demonstrates persistent plugin state).
    SwapCount,
}

// ── Errors ────────────────────────────────────────────────────────────────────

#[contracterror]
#[derive(Copy, Clone, Debug, PartialEq)]
pub enum PluginError {
    AlreadyInitialized = 1,
    NotInitialized = 2,
    Unauthorized = 3,
    /// `token_in` and `token_out` must be different addresses.
    SameToken = 4,
    /// `amount_in` must be positive.
    InvalidAmount = 5,
    /// `fee_bps` must be within `[0, 10_000)`.
    InvalidFeeBps = 6,
    /// The computed `amount_out` is below the caller's `min_amount_out`.
    InsufficientOutput = 7,
}

// ── Contract ──────────────────────────────────────────────────────────────────

#[contract]
pub struct FixedRateLiquidityPlugin;

#[contractimpl]
impl FixedRateLiquidityPlugin {
    /// Initialize the plugin with an admin address.
    ///
    /// Sets the admin who can tweak the fee. This is optional — the plugin is
    /// still fully usable (with the default fee) before initialization.
    ///
    /// # Arguments
    /// * `env` — The Soroban environment.
    /// * `admin` — The address that will hold admin privileges over this plugin.
    ///
    /// # Returns
    /// `Ok(())` on success.
    ///
    /// # Errors
    /// * [`PluginError::AlreadyInitialized`] — if the plugin has already been initialized.
    pub fn initialize(env: Env, admin: Address) -> Result<(), PluginError> {
        if env.storage().instance().has(&DataKey::Admin) {
            return Err(PluginError::AlreadyInitialized);
        }
        env.storage().instance().set(&DataKey::Admin, &admin);
        env.storage().instance().set(&DataKey::SwapCount, &0u64);
        Ok(())
    }

    /// Return the plugin's well-known name for router-registry registration.
    ///
    /// # Arguments
    /// * `env` — The Soroban environment.
    ///
    /// # Returns
    /// `"liquidity/example-fixed-rate"`.
    pub fn name(env: Env) -> String {
        String::from_str(&env, PLUGIN_NAME)
    }

    /// Return the plugin's version, reported alongside its name.
    ///
    /// # Arguments
    /// * `env` — The Soroban environment.
    ///
    /// # Returns
    /// The plugin version as a `u32`.
    pub fn version(_env: Env) -> u32 {
        PLUGIN_VERSION
    }

    /// Quote an input amount into an output amount for a token pair.
    ///
    /// Applies the configured fee: `amount_out = amount_in * (10_000 - fee_bps) / 10_000`.
    /// Emits a `quote_generated` event.
    ///
    /// # Arguments
    /// * `env` — The Soroban environment.
    /// * `token_in` — The address of the input token.
    /// * `token_out` — The address of the output token.
    /// * `amount_in` — The amount of input token (must be positive).
    ///
    /// # Returns
    /// The expected output amount after fees.
    ///
    /// # Errors
    /// * [`PluginError::SameToken`] — if `token_in` equals `token_out`.
    /// * [`PluginError::InvalidAmount`] — if `amount_in` is not positive.
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

        let fee_bps = Self::fee_bps(env.clone());
        let amount_out = amount_in * (10_000 - fee_bps) / 10_000;

        env.events().publish(
            (Symbol::new(&env, "quote_generated"),),
            (token_in, token_out, amount_in, amount_out, fee_bps),
        );

        Ok(amount_out)
    }

    /// "Execute" a swap for the given input amount, enforcing slippage control.
    ///
    /// In this reference implementation the swap is simulated: the plugin
    /// computes the output exactly as [`get_quote`](Self::get_quote) does and
    /// never moves tokens. A real plugin would instead transfer `amount_in`
    /// in and `amount_out` out using the token contracts. `execute_swap`
    /// fails if the resulting `amount_out` is below `min_amount_out`.
    ///
    /// # Arguments
    /// * `env` — The Soroban environment.
    /// * `caller` — The address initiating the swap; must authenticate via
    ///   Soroban authorization.
    /// * `token_in` — The address of the input token.
    /// * `token_out` — The address of the output token.
    /// * `amount_in` — The amount of input token.
    /// * `min_amount_out` — The minimum acceptable output amount (slippage guard).
    ///
    /// # Returns
    /// The actual output amount.
    ///
    /// # Errors
    /// * [`PluginError::SameToken`] — if `token_in` equals `token_out`.
    /// * [`PluginError::InvalidAmount`] — if `amount_in` is not positive.
    /// * [`PluginError::InsufficientOutput`] — if the computed output is below `min_amount_out`.
    pub fn execute_swap(
        env: Env,
        caller: Address,
        token_in: Address,
        token_out: Address,
        amount_in: i128,
        min_amount_out: i128,
    ) -> Result<i128, PluginError> {
        caller.require_auth();

        let amount_out =
            Self::get_quote(env.clone(), token_in.clone(), token_out.clone(), amount_in)?;

        if amount_out < min_amount_out {
            return Err(PluginError::InsufficientOutput);
        }

        let count: u64 = env
            .storage()
            .instance()
            .get(&DataKey::SwapCount)
            .unwrap_or(0);
        env.storage()
            .instance()
            .set(&DataKey::SwapCount, &(count + 1));

        env.events().publish(
            (Symbol::new(&env, "swap_executed"),),
            (caller, token_in, token_out, amount_in, amount_out),
        );

        Ok(amount_out)
    }

    /// Set a custom fee for the plugin.
    ///
    /// The caller must be the plugin admin. `fee_bps` must be in `[0, 10_000)`.
    ///
    /// # Arguments
    /// * `env` — The Soroban environment.
    /// * `caller` — The address initiating the call; must be the admin.
    /// * `fee_bps` — The new fee in basis points.
    ///
    /// # Returns
    /// `Ok(())` on success.
    ///
    /// # Errors
    /// * [`PluginError::NotInitialized`] — if the plugin has no admin set.
    /// * [`PluginError::Unauthorized`] — if `caller` is not the admin.
    /// * [`PluginError::InvalidFeeBps`] — if `fee_bps` is outside `[0, 10_000)`.
    pub fn set_fee(env: Env, caller: Address, fee_bps: i128) -> Result<(), PluginError> {
        caller.require_auth();
        router_common::require_admin_simple!(&env, &caller, &DataKey::Admin, PluginError)?;

        if !(0..10_000).contains(&fee_bps) {
            return Err(PluginError::InvalidFeeBps);
        }

        env.storage().instance().set(&DataKey::FeeBps, &fee_bps);

        env.events()
            .publish((Symbol::new(&env, "fee_updated"),), (caller, fee_bps));

        Ok(())
    }

    /// Get the fee currently applied to quotes and swaps, in basis points.
    ///
    /// Returns [`DEFAULT_FEE_BPS`] when no custom fee has been set.
    ///
    /// # Arguments
    /// * `env` — The Soroban environment.
    ///
    /// # Returns
    /// The fee in basis points.
    pub fn fee_bps(env: Env) -> i128 {
        env.storage()
            .instance()
            .get(&DataKey::FeeBps)
            .unwrap_or(DEFAULT_FEE_BPS)
    }

    /// Get the total number of swaps "executed" by this plugin.
    ///
    /// # Arguments
    /// * `env` — The Soroban environment.
    ///
    /// # Returns
    /// The cumulative swap count.
    pub fn swap_count(env: Env) -> u64 {
        env.storage()
            .instance()
            .get(&DataKey::SwapCount)
            .unwrap_or(0)
    }

    /// Get the current plugin admin.
    ///
    /// # Arguments
    /// * `env` — The Soroban environment.
    ///
    /// # Returns
    /// The admin address, or [`PluginError::NotInitialized`] if none is set.
    pub fn admin(env: Env) -> Result<Address, PluginError> {
        env.storage()
            .instance()
            .get(&DataKey::Admin)
            .ok_or(PluginError::NotInitialized)
    }

    /// Transfer admin to a new address.
    ///
    /// # Arguments
    /// * `env` — The Soroban environment.
    /// * `current` — The current admin address; must authenticate.
    /// * `new_admin` — The address that will become the new admin.
    ///
    /// # Returns
    /// `Ok(())` on success.
    ///
    /// # Errors
    /// * [`PluginError::NotInitialized`] — if the plugin has no admin set.
    /// * [`PluginError::Unauthorized`] — if `current` is not the admin.
    pub fn transfer_admin(
        env: Env,
        current: Address,
        new_admin: Address,
    ) -> Result<(), PluginError> {
        current.require_auth();
        router_common::require_admin_simple!(&env, &current, &DataKey::Admin, PluginError)?;
        router_common::admin_transfer_complete!(&env, &current, &new_admin, &DataKey::Admin);
        Ok(())
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    extern crate std;
    use super::*;
    use soroban_sdk::{
        testutils::{Address as _, Events},
        Env, Symbol, TryFromVal,
    };

    fn setup() -> (Env, Address, FixedRateLiquidityPluginClient<'static>) {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register_contract(None, FixedRateLiquidityPlugin);
        let client = FixedRateLiquidityPluginClient::new(&env, &contract_id);
        let admin = Address::generate(&env);
        (env, admin, client)
    }

    fn setup_initialized() -> (Env, Address, FixedRateLiquidityPluginClient<'static>) {
        let (env, admin, client) = setup();
        client.initialize(&admin);
        (env, admin, client)
    }

    #[test]
    fn test_name_and_version() {
        let (env, _admin, client) = setup();
        assert_eq!(client.name(), String::from_str(&env, PLUGIN_NAME));
        assert_eq!(client.version(), PLUGIN_VERSION);
    }

    #[test]
    fn test_get_quote_default_fee() {
        let (env, _admin, client) = setup();
        let token_in = Address::generate(&env);
        let token_out = Address::generate(&env);
        // amount_out = 1_000 * (10_000 - 30) / 10_000 = 997
        assert_eq!(client.get_quote(&token_in, &token_out, &1_000), 997);
    }

    #[test]
    fn test_get_quote_custom_fee() {
        let (env, admin, client) = setup_initialized();
        client.set_fee(&admin, &100); // 1%
        let token_in = Address::generate(&env);
        let token_out = Address::generate(&env);
        // amount_out = 1_000 * (10_000 - 100) / 10_000 = 990
        assert_eq!(client.get_quote(&token_in, &token_out, &1_000), 990);
        assert_eq!(client.fee_bps(), 100);
    }

    #[test]
    fn test_get_quote_same_token_fails() {
        let (env, _admin, client) = setup();
        let token = Address::generate(&env);
        let result = client.try_get_quote(&token, &token, &1_000);
        assert_eq!(result, Err(Ok(PluginError::SameToken)));
    }

    #[test]
    fn test_get_quote_invalid_amount_fails() {
        let (env, _admin, client) = setup();
        let token_in = Address::generate(&env);
        let token_out = Address::generate(&env);
        let result = client.try_get_quote(&token_in, &token_out, &0);
        assert_eq!(result, Err(Ok(PluginError::InvalidAmount)));
    }

    #[test]
    fn test_execute_swap_success() {
        let (env, _admin, client) = setup_initialized();
        let caller = Address::generate(&env);
        let token_in = Address::generate(&env);
        let token_out = Address::generate(&env);
        // default fee 30 bps → 1_000 → 997
        assert_eq!(
            client.execute_swap(&caller, &token_in, &token_out, &1_000, &997),
            997
        );
        assert_eq!(client.swap_count(), 1);

        let swap_event_emitted = env.events().all().iter().any(|e| {
            e.1.get(0)
                .and_then(|v| Symbol::try_from_val(&env, &v).ok())
                .map(|s| s == Symbol::new(&env, "swap_executed"))
                .unwrap_or(false)
        });
        assert!(swap_event_emitted, "swap_executed event must be emitted");
    }

    #[test]
    fn test_execute_swap_min_amount_out_fails() {
        let (env, _admin, client) = setup_initialized();
        let caller = Address::generate(&env);
        let token_in = Address::generate(&env);
        let token_out = Address::generate(&env);
        // amount_out = 997, so min_amount_out = 998 must fail
        let result = client.try_execute_swap(&caller, &token_in, &token_out, &1_000, &998);
        assert_eq!(result, Err(Ok(PluginError::InsufficientOutput)));
        assert_eq!(client.swap_count(), 0);
    }

    #[test]
    fn test_execute_swap_requires_auth() {
        let env = Env::default();
        let contract_id = env.register_contract(None, FixedRateLiquidityPlugin);
        let client = FixedRateLiquidityPluginClient::new(&env, &contract_id);
        let caller = Address::generate(&env);
        let token_in = Address::generate(&env);
        let token_out = Address::generate(&env);
        // With auth mocked OFF, an unauthenticated caller must be rejected.
        let result = client.try_execute_swap(&caller, &token_in, &token_out, &1_000, &0);
        assert!(result.is_err(), "unauthenticated execute_swap must fail");
    }

    #[test]
    fn test_initialize_twice_fails() {
        let (_env, admin, client) = setup();
        client.initialize(&admin);
        let result = client.try_initialize(&admin);
        assert_eq!(result, Err(Ok(PluginError::AlreadyInitialized)));
    }

    #[test]
    fn test_set_fee_unauthorized_fails() {
        let (env, _admin, client) = setup_initialized();
        let attacker = Address::generate(&env);
        let result = client.try_set_fee(&attacker, &50);
        assert_eq!(result, Err(Ok(PluginError::Unauthorized)));
        assert_eq!(client.fee_bps(), DEFAULT_FEE_BPS, "fee must be unchanged");
    }

    #[test]
    fn test_set_fee_invalid_fails() {
        let (_env, admin, client) = setup_initialized();
        let result = client.try_set_fee(&admin, &10_000);
        assert_eq!(result, Err(Ok(PluginError::InvalidFeeBps)));
    }

    #[test]
    fn test_transfer_admin() {
        let (env, admin, client) = setup_initialized();
        let new_admin = Address::generate(&env);
        client.transfer_admin(&admin, &new_admin);
        assert_eq!(client.admin(), new_admin);
    }

    #[test]
    fn test_admin_missing_returns_not_initialized() {
        let (_env, _admin, client) = setup();
        assert_eq!(client.try_admin(), Err(Ok(PluginError::NotInitialized)));
    }
}

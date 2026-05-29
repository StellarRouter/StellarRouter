#![no_std]

//! # router-quote
//!
//! Preview transaction results before execution. Supports single-hop and
//! multi-hop quotes where the output of one pool feeds into the next.
//!
//! ## Events (following naming convention: past tense verbs in snake_case)
//! - `quote_generated` — Quote computed (amount_in, amount_out, exchange_rate)
//! - `fee_estimated`   — Fee estimation completed (total_fee, surge_pricing)
//! - `admin_transferred` — Admin transferred (old_admin, new_admin)

use soroban_sdk::{
    contract, contracterror, contractimpl, contracttype, Address, Env, IntoVal, Symbol,
    TryFromVal, Vec,
};

// ── Storage Keys ──────────────────────────────────────────────────────────────

#[contracttype]
pub enum DataKey {
    Admin,
    QuoteTtl,
    QuoteTtl, // TTL for quotes in ledger seconds
    HopCache(Address, Address, Address, i128), // (plugin, token_in, token_out, amount_in) -> HopCacheEntry
}

// ── Types ─────────────────────────────────────────────────────────────────────

/// A single hop in a multi-hop route.
#[contracttype]
#[derive(Clone, Debug, PartialEq)]
pub struct HopDescriptor {
    pub plugin: Address,
    pub token_in: Address,
    pub token_out: Address,
    /// Fee rate for this hop in basis points (e.g. 30 = 0.30%).
    pub fee_bps: u32,
}

/// Result of a single hop.
#[contracttype]
#[derive(Clone, Debug, PartialEq)]
pub struct HopResult {
    pub token_in: Address,
    pub token_out: Address,
    pub amount_in: i128,
    pub amount_out: i128,
    pub fee_amount: i128,
}

#[contracttype]
#[derive(Clone, Debug, PartialEq)]
pub struct HopCacheEntry {
    pub amount_out: i128,
    pub expires_at: u64,
}

/// Response for a single-hop or multi-hop quote.
#[contracttype]
#[derive(Clone, Debug, PartialEq)]
pub struct QuoteResponse {
    /// Final output amount after all hops.
    pub amount_out: i128,
    /// Total fees across all hops (in token_in units of each hop).
    pub total_fee_amount: i128,
    /// Minimum acceptable output after slippage tolerance.
    pub min_amount_out: i128,
    /// Exchange rate as fixed-point: (amount_out * 10^precision) / amount_in.
    pub exchange_rate: i128,
    /// Decimal places in `exchange_rate`.
    pub precision: u32,
    /// Price impact in basis points (negative = adverse).
    pub price_impact_bps: i32,
    /// Per-hop breakdown.
    pub hops: Vec<HopResult>,
}

/// Request parameters for fee estimation.
#[contracttype]
#[derive(Clone, Debug, PartialEq)]
pub struct FeeEstimateRequest {
    /// Amount of token_in being transacted (must be > 0).
    pub amount: i128,
    /// Fee rate in basis points charged by the route (e.g., 30 = 0.30%).
    pub fee_bps: u32,
    /// Current network utilization in basis points (0–10000).
    /// Values ≥ 8000 trigger surge pricing.
    pub network_load_bps: u32,
}

/// Estimated fee breakdown for a transaction.
#[contracttype]
#[derive(Clone, Debug, PartialEq)]
pub struct FeeEstimateResponse {
    /// Protocol fee charged by the route (in token_in base units).
    pub protocol_fee: i128,
    /// Network/gas fee in stroops.
    pub network_fee: i128,
    /// Total estimated fee (protocol + network).
    pub total_fee: i128,
    /// Whether surge pricing was applied due to high network load.
    pub surge_pricing: bool,
    /// Effective fee rate in basis points after surge adjustment.
    pub effective_fee_bps: u32,
}

// ── Errors ────────────────────────────────────────────────────────────────────

#[contracterror]
#[derive(Copy, Clone, Debug, PartialEq)]
pub enum QuoteError {
    AlreadyInitialized = 1,
    NotInitialized = 2,
    Unauthorized = 3,
    InvalidAmount = 4,
    QuoteFailed = 5,
    InvalidPrecision = 6,
    InvalidSlippage = 7,
    EmptyRoute = 8,
    RouteTooLong = 9,
    InvalidAmount = 1,
    RouteNotFound = 2,
    QuoteFailed = 3,
    InvalidPrecision = 4,
    InvalidSlippage = 5,
    EmptyRoute = 6,
    RouteTooLong = 7,
    TokenMismatch = 8,
}

/// Maximum hops allowed in a multi-hop route.
const MAX_HOPS: u32 = 5;
const HOP_CACHE_TTL_SECS: u64 = 5;

// ── Contract ──────────────────────────────────────────────────────────────────

#[contract]
pub struct RouterQuote;

#[contractimpl]
impl RouterQuote {
    // ── Admin ─────────────────────────────────────────────────────────────────

    /// Initialize the contract with an admin address.
    ///
    /// # Errors
    /// * [`QuoteError::AlreadyInitialized`] — called more than once.
    pub fn initialize(env: Env, admin: Address) -> Result<(), QuoteError> {
        if env.storage().instance().has(&DataKey::Admin) {
            return Err(QuoteError::AlreadyInitialized);
        }
        env.storage().instance().set(&DataKey::Admin, &admin);
        Ok(())
    }

    /// Return the current admin address.
    /// Get the current admin address.
    ///
    /// # Errors
    /// Returns `QuoteError::NotInitialized` if the contract has not been initialized.
    pub fn admin(env: Env) -> Result<Address, QuoteError> {
        env.storage()
            .instance()
            .get(&DataKey::Admin)
            .ok_or(QuoteError::NotInitialized)
    }

    /// Transfer admin to a new address.
    ///
    /// # Errors
    /// * [`QuoteError::NotInitialized`] — contract not initialized.
    /// * [`QuoteError::Unauthorized`]   — `current` is not the admin.
    pub fn transfer_admin(
        env: Env,
        current: Address,
        new_admin: Address,
    ) -> Result<(), QuoteError> {
        current.require_auth();
        let admin: Address = env
            .storage()
            .instance()
            .get(&DataKey::Admin)
            .ok_or(QuoteError::NotInitialized)?;
        if admin != current {
            return Err(QuoteError::Unauthorized);
        }
        router_common::admin_transfer_complete!(&env, &current, &new_admin, &DataKey::Admin);
        Ok(())
    }

    /// Set the quote TTL (seconds). Admin only.
    pub fn set_quote_ttl(env: Env, caller: Address, ttl: u64) -> Result<(), QuoteError> {
        caller.require_auth();
        let admin: Address = env
            .storage()
            .instance()
            .get(&DataKey::Admin)
            .ok_or(QuoteError::NotInitialized)?;
        if admin != caller {
            return Err(QuoteError::Unauthorized);
        }
        env.storage().instance().set(&DataKey::QuoteTtl, &ttl);
        Ok(())
    }

    /// Get the current quote TTL (seconds). Defaults to 300.
    pub fn get_quote_ttl(env: Env) -> u64 {
        env.storage().instance().get(&DataKey::QuoteTtl).unwrap_or(300)
    }

    // ── Quote functions ───────────────────────────────────────────────────────

    /// Get a single-hop quote from a liquidity plugin.
    ///
    /// Calls `get_quote(token_in, token_out, amount_in) -> i128` on `plugin`.
    ///
    /// # Errors
    /// * [`QuoteError::InvalidAmount`]    — `amount_in` ≤ 0.
    /// * [`QuoteError::InvalidPrecision`] — `precision` is 0 or > 18.
    /// * [`QuoteError::InvalidSlippage`]  — `slippage_bps` > 10000.
    /// * [`QuoteError::QuoteFailed`]      — plugin call failed.
    pub fn get_quote(
        env: Env,
        plugin: Address,
        token_in: Address,
        token_out: Address,
        amount_in: i128,
        fee_bps: u32,
        slippage_bps: u32,
        precision: u32,
    ) -> Result<QuoteResponse, QuoteError> {
        if amount_in <= 0 {
            return Err(QuoteError::InvalidAmount);
        }
        if precision == 0 || precision > 18 {
            return Err(QuoteError::InvalidPrecision);
        }
        if slippage_bps > 10_000 {
            return Err(QuoteError::InvalidSlippage);
        }
        let mut hops = Vec::new(&env);
        hops.push_back(HopDescriptor { plugin, token_in, token_out, fee_bps });
        Self::execute_hops(&env, hops, amount_in, slippage_bps, precision)
    }

    /// Get a multi-hop quote chaining N liquidity plugins (max 5).
    ///
    /// # Errors
    /// * [`QuoteError::EmptyRoute`]       — `hops` is empty.
    /// * [`QuoteError::RouteTooLong`]     — more than `MAX_HOPS` hops.
    /// * [`QuoteError::InvalidAmount`]    — `amount_in` ≤ 0.
    /// * [`QuoteError::InvalidPrecision`] — `precision` is 0 or > 18.
    /// * [`QuoteError::InvalidSlippage`]  — `slippage_bps` > 10000.
    /// * [`QuoteError::QuoteFailed`]      — any plugin call failed.
    pub fn get_multihop_quote(
        env: Env,
        hops: Vec<HopDescriptor>,
        amount_in: i128,
        slippage_bps: u32,
        precision: u32,
    ) -> Result<QuoteResponse, QuoteError> {
        if hops.is_empty() {
            return Err(QuoteError::EmptyRoute);
        }
        if hops.len() > MAX_HOPS {
            return Err(QuoteError::RouteTooLong);
        }
        if amount_in <= 0 {
            return Err(QuoteError::InvalidAmount);
        }
        if precision == 0 || precision > 18 {
            return Err(QuoteError::InvalidPrecision);
        }
        if slippage_bps > 10_000 {
            return Err(QuoteError::InvalidSlippage);
        }

        // Validate token continuity: hop[N].token_out must equal hop[N+1].token_in
        let hop_count = hops.len();
        let mut i = 0u32;
        while i + 1 < hop_count {
            let current = hops.get(i).unwrap();
            let next = hops.get(i + 1).unwrap();
            if current.token_out != next.token_in {
                return Err(QuoteError::TokenMismatch);
            }
        };

        // Try to invoke the get_quote function on the target contract
        // The plugin interface expects: get_quote(token_in, token_out, amount_in) -> i128
        let function = Symbol::new(&env, "get_quote");
        
        // Build args: (token_in, token_out, amount_in)
        let mut args = Vec::new(&env);
        args.push_back(token_in.into());
        args.push_back(token_out.into());
        args.push_back(amount_in.into());

        let amount_out: i128 = env
            .try_invoke_contract::<i128, i128>(&target, &function, args)
            .map_err(|_| QuoteError::QuoteFailed)?
            .map_err(|_| QuoteError::QuoteFailed)?;

        // Protocol fee: amount_in * fee_bps / 10_000
        let fee_amount = amount_in * fee_bps as i128 / 10_000;

        // Slippage: min_amount_out = amount_out * (10_000 - slippage_bps) / 10_000
        let min_amount_out = amount_out * (10_000 - slippage_bps as i128) / 10_000;

        // Exchange rate as fixed-point: (amount_out * 10^precision) / amount_in
        // Uses i128 arithmetic — safe for precision ≤ 18 and typical token amounts.
        let scale = Self::pow10(precision);
        let exchange_rate = (amount_out * scale) / amount_in;

        // Price impact: simplified as (amount_out - amount_in) * 10_000 / amount_in
        // Negative means the user receives less than they put in (adverse).
        let price_impact_bps = ((amount_out - amount_in) * 10_000 / amount_in) as i32;

        env.events().publish(
            (Symbol::new(&env, "quote_generated"),),
            (&target, amount_in, amount_out, exchange_rate),
        );
        // Attempt the cross-contract call
        let amount_out: i128 = env
            .invoke_contract(&target, &function, args);

        // Calculate fee (assuming 1% fee for now - in production this comes from the plugin)
        let fee_amount = amount_in * 1 / 100;
        
        // Calculate min_amount_out using caller-specified slippage_bps
        // Formula: amount_out * (10000 - slippage_bps) / 10000
        let min_amount_out = amount_out * (10_000 - slippage_bps as i128) / 10_000;
        
        // Exchange rate placeholder
        let exchange_rate = String::from_str(&env, "0");

        // Price impact: (amount_out - amount_in) * 10_000 / amount_in
        // Negative means the user receives less than they put in (adverse).
        let price_impact_bps = ((amount_out - amount_in) * 10_000 / amount_in) as i32;
            i += 1;
        }

        Self::execute_hops(&env, hops, amount_in, slippage_bps, precision)
    }

    /// Get the best quote from multiple candidate routes.
    ///
    /// Runs `get_multihop_quote` for each route in `hops_list` and returns
    /// the route with the highest `amount_out`. Routes that fail or produce
    /// zero output are skipped. Returns an error if no valid quote is found.
    ///
    /// # Arguments
    /// * `env` - The Soroban environment.
    /// * `hops_list` - A list of candidate routes, each a `Vec<HopDescriptor>`.
    /// * `amount_in` - The input amount for all routes.
    /// * `slippage_bps` - Slippage tolerance in basis points.
    /// * `precision` - Decimal precision for exchange rate.
    ///
    /// # Returns
    /// The [`QuoteResponse`] with the highest `amount_out`.
    ///
    /// # Errors
    /// * [`QuoteError::EmptyRoute`] — if `hops_list` is empty or all routes fail.
    pub fn get_best_quote(
        env: Env,
        hops_list: Vec<Vec<HopDescriptor>>,
        amount_in: i128,
        slippage_bps: u32,
        precision: u32,
    ) -> Result<QuoteResponse, QuoteError> {
        if hops_list.is_empty() {
            return Err(QuoteError::EmptyRoute);
        }

        let mut best: Option<QuoteResponse> = None;

        for hops in hops_list.iter() {
            if let Ok(quote) =
                Self::get_multihop_quote(env.clone(), hops, amount_in, slippage_bps, precision)
            {
                if quote.amount_out > 0 {
                    let is_better = match &best {
                        None => true,
                        Some(b) => quote.amount_out > b.amount_out,
                    };
                    if is_better {
                        best = Some(quote);
                    }
                }
            }
        }

        best.ok_or(QuoteError::EmptyRoute)
    }

    /// Estimate fees for a single transaction.
    ///
    /// # Errors
    /// * [`QuoteError::InvalidAmount`] — `request.amount` ≤ 0.
    pub fn estimate_fee(
        env: Env,
        request: FeeEstimateRequest,
    ) -> Result<FeeEstimateResponse, QuoteError> {
        if request.amount <= 0 {
            return Err(QuoteError::InvalidAmount);
        }
        let protocol_fee = request.amount * request.fee_bps as i128 / 10_000;
        let base_network_fee: i128 = 100;
        let (network_fee, surge_pricing, effective_fee_bps) = if request.network_load_bps >= 8_000 {
            (base_network_fee * 2, true, request.fee_bps * 2)
        } else {
            (base_network_fee, false, request.fee_bps)
        };
        env.events().publish(
            (Symbol::new(&env, "fee_estimated"),),
            (protocol_fee + network_fee, surge_pricing),
        );
        Ok(FeeEstimateResponse {
            protocol_fee,
            network_fee,
            total_fee: protocol_fee + network_fee,
            surge_pricing,
            effective_fee_bps,
        })
    }

    /// Estimate fees for multiple transactions. Invalid requests are skipped.
    pub fn estimate_fees(
        env: Env,
        requests: Vec<FeeEstimateRequest>,
    ) -> Vec<FeeEstimateResponse> {
        let mut responses = Vec::new(&env);
        for req in requests.iter() {
            if let Ok(estimate) = Self::estimate_fee(env.clone(), req) {
                responses.push_back(estimate);
            }
        }
        responses
    }

    // ── Helpers ───────────────────────────────────────────────────────────────

    fn execute_hops(
        env: &Env,
        hops: Vec<HopDescriptor>,
        initial_amount_in: i128,
        slippage_bps: u32,
        precision: u32,
    ) -> Result<QuoteResponse, QuoteError> {
        let mut current_amount = initial_amount_in;
        let mut total_fee: i128 = 0;
        let mut hop_results = Vec::new(env);

        for hop in hops.iter() {
            let gross_amount_out =
                Self::call_plugin(env, &hop.plugin, &hop.token_in, &hop.token_out, current_amount)?;
            let gross_amount_out = Self::get_cached_hop_quote(
                env,
                &hop.plugin,
                &hop.token_in,
                &hop.token_out,
                current_amount,
            )?;

            // Fee is taken from the input of each hop
            let fee_amount = current_amount * hop.fee_bps as i128 / 10_000;
            total_fee += fee_amount;
            hop_results.push_back(HopResult {
                token_in: hop.token_in.clone(),
                token_out: hop.token_out.clone(),
                amount_in: current_amount,
                amount_out: gross_amount_out,
                fee_amount,
            });
            current_amount = gross_amount_out;
        }

        let final_amount_out = current_amount;
        let min_amount_out = final_amount_out * (10_000 - slippage_bps as i128) / 10_000;
        let scale = Self::pow10(precision);
        let exchange_rate = (final_amount_out * scale) / initial_amount_in;
        let price_impact_bps =
            ((final_amount_out - initial_amount_in) * 10_000 / initial_amount_in) as i32;

        env.events().publish(
            (Symbol::new(env, "quote_generated"),),
            (initial_amount_in, final_amount_out, exchange_rate),
        );

        Ok(QuoteResponse {
            amount_out: final_amount_out,
            total_fee_amount: total_fee,
            min_amount_out,
            exchange_rate,
            precision,
            price_impact_bps,
            hops: hop_results,
        })
    }

    fn call_plugin(
        env: &Env,
        plugin: &Address,
        token_in: &Address,
        token_out: &Address,
        amount_in: i128,
    ) -> Result<i128, QuoteError> {
        let function = Symbol::new(env, "get_quote");
        let mut args: Vec<soroban_sdk::Val> = Vec::new(env);
        args.push_back(token_in.clone().into_val(env));
        args.push_back(token_out.clone().into_val(env));
        args.push_back(amount_in.into_val(env));
        let result = env
            .try_invoke_contract::<soroban_sdk::Val, soroban_sdk::Val>(plugin, &function, args)
            .map_err(|_| QuoteError::QuoteFailed)?
            .map_err(|_| QuoteError::QuoteFailed)?;
        i128::try_from_val(env, &result).map_err(|_| QuoteError::QuoteFailed)
            .map_err(|_| QuoteError::QuoteFailed)
    }

    /// Returns a hop quote, using a short-lived per-hop cache when available.
    fn get_cached_hop_quote(
        env: &Env,
        plugin: &Address,
        token_in: &Address,
        token_out: &Address,
        amount_in: i128,
    ) -> Result<i128, QuoteError> {
        let key = DataKey::HopCache(
            plugin.clone(),
            token_in.clone(),
            token_out.clone(),
            amount_in,
        );
        let now = env.ledger().timestamp();

        if let Some(cached) = env.storage().instance().get::<DataKey, HopCacheEntry>(&key) {
            if now < cached.expires_at {
                return Ok(cached.amount_out);
            }
            env.storage().instance().remove(&key);
        }

        let amount_out = Self::call_plugin(env, plugin, token_in, token_out, amount_in)?;
        env.storage().instance().set(
            &key,
            &HopCacheEntry {
                amount_out,
                expires_at: now + HOP_CACHE_TTL_SECS,
            },
        );

        Ok(amount_out)
    }
    /// Get multiple quotes in a single call (for comparing routes).
    ///
    /// # Arguments
    /// * `env` - The Soroban environment.
    /// * `router_core` - Optional address of router-core contract for route resolution.
    /// * `requests` - A vector of [`QuoteRequest`]s to process.
    ///
    /// # Returns
    /// A vector of [`QuoteResponse`]s (one per request). Failed quotes
    /// will have `amount_out = 0` and an appropriate error handling strategy.
    pub fn get_quotes(
        env: Env,
        router_core: Option<Address>,
        requests: Vec<QuoteRequest>,
    ) -> Vec<Result<QuoteResponse, QuoteError>> {
        let mut responses = Vec::new(&env);
        for req in requests.iter() {
            let result = Self::get_quote(
                env.clone(),
                router_core.clone(),
                req.route_name.clone(),
                req.token_in.clone(),
                req.token_out.clone(),
                req.amount_in,
                req.slippage_bps,
            );
            responses.push_back(result);
        }
        responses
    }

    /// Estimate fees for a single transaction.
    ///
    /// Computes protocol and network fees based on the transaction amount,
    /// the route's fee rate, and current network load. Surge pricing (2×
    /// network fee) is applied when `network_load_bps` ≥ 8000 (80%).
    ///
    /// # Arguments
    /// * `env` - The Soroban environment.
    /// * `request` - A [`FeeEstimateRequest`] describing the transaction parameters.
    ///
    /// # Returns
    /// A [`FeeEstimateResponse`] with a full fee breakdown.
    ///
    /// # Errors
    /// * [`QuoteError::InvalidAmount`] — if `request.amount` ≤ 0.
    pub fn estimate_fee(env: Env, request: FeeEstimateRequest) -> Result<FeeEstimateResponse, QuoteError> {
        if request.amount <= 0 {
            return Err(QuoteError::InvalidAmount);
        }
        if request.fee_bps > 10_000 {
            return Err(QuoteError::InvalidFee);
        }

        // Protocol fee: amount * fee_bps / 10000
        let protocol_fee = request.amount * request.fee_bps as i128 / 10_000;

        // Base network fee: 100 stroops minimum
        let base_network_fee: i128 = 100;

        // Surge pricing at ≥ 80% network load
        let (network_fee, surge_pricing, effective_fee_bps) = if request.network_load_bps >= 8_000 {
            (base_network_fee * 2, true, request.fee_bps * 2)
        } else {
            (base_network_fee, false, request.fee_bps)
        };

        let total_fee = protocol_fee + network_fee;

        env.events().publish(
            (Symbol::new(&env, "fee_estimated"),),
            (total_fee, surge_pricing),
        );

        Ok(FeeEstimateResponse {
            protocol_fee,
            network_fee,
            total_fee,
            surge_pricing,
            effective_fee_bps,
        })
    }

    fn pow10(exp: u32) -> i128 {
        let mut result: i128 = 1;
        let mut i = 0u32;
        while i < exp {
            result *= 10;
            i += 1;
        }
        result
    }

    /// Compare quotes across multiple routes and return the best one after
    /// filtering out routes whose price impact exceeds `max_price_impact_bps`.
    ///
    /// "Best" is defined as the highest `amount_out` among the remaining routes.
    ///
    /// # Arguments
    /// * `routes`               — Pre-computed quotes to compare (e.g. from `get_quote`).
    /// * `max_price_impact_bps` — Maximum acceptable price impact in basis points
    ///                            (e.g. 100 = 1%). Routes with `price_impact_bps`
    ///                            more negative than `-max_price_impact_bps` are excluded.
    ///
    /// # Errors
    /// * [`QuoteError::EmptyRoute`]   — `routes` is empty.
    /// * [`QuoteError::QuoteFailed`]  — all routes were filtered out by the threshold.
    pub fn compare_quotes(
        env: Env,
        routes: Vec<QuoteResponse>,
        max_price_impact_bps: u32,
    ) -> Result<QuoteResponse, QuoteError> {
        if routes.is_empty() {
            return Err(QuoteError::EmptyRoute);
        }

        let threshold = -(max_price_impact_bps as i32);
        let mut best: Option<QuoteResponse> = None;

        for quote in routes.iter() {
            if quote.price_impact_bps < threshold {
                continue;
            }
            match &best {
                None => best = Some(quote),
                Some(b) if quote.amount_out > b.amount_out => best = Some(quote),
                _ => {}
            }
        }

        best.ok_or(QuoteError::QuoteFailed)
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use soroban_sdk::{testutils::{Address as _, Events as _}, Env, IntoVal};

    mod double_plugin {
        use soroban_sdk::{contract, contractimpl, Address, Env};
        #[contract]
        pub struct DoublePlugin;
        #[contractimpl]
        impl DoublePlugin {
            pub fn get_quote(_env: Env, _ti: Address, _to: Address, amount_in: i128) -> i128 {
                amount_in * 2
            }
        }
    }

    mod triple_plugin {
        use soroban_sdk::{contract, contractimpl, Address, Env};
        #[contract]
        pub struct TriplePlugin;
        #[contractimpl]
        impl TriplePlugin {
            pub fn get_quote(_env: Env, _ti: Address, _to: Address, amount_in: i128) -> i128 {
                amount_in * 3
            }
        }
    }

    use double_plugin::DoublePlugin;
    use triple_plugin::TriplePlugin;

    fn setup() -> (Env, RouterQuoteClient<'static>, Address, Address) {
        let env = Env::default();
        env.mock_all_auths();
        let id = env.register_contract(None, RouterQuote);
        let client = RouterQuoteClient::new(&env, &id);
        let double = env.register_contract(None, DoublePlugin);
        let triple = env.register_contract(None, TriplePlugin);
        (env, client, double, triple)
    }

    // ── Admin tests ───────────────────────────────────────────────────────────

    #[test]
    fn test_initialize_sets_admin() {
        let (env, client, _, _) = setup();
        let admin = Address::generate(&env);
        client.initialize(&admin);
        assert_eq!(client.admin(), admin);
    }

    #[test]
    fn test_initialize_twice_fails() {
        let (env, client, _, _) = setup();
        let admin = Address::generate(&env);
        client.initialize(&admin);
        let result = client.try_initialize(&admin);
        assert_eq!(result, Err(Ok(QuoteError::AlreadyInitialized)));
    }

    #[test]
    fn test_transfer_admin() {
        let (env, client, _, _) = setup();
        let admin = Address::generate(&env);
        let new_admin = Address::generate(&env);
        client.initialize(&admin);
        client.transfer_admin(&admin, &new_admin);
        assert_eq!(client.admin(), new_admin);
    }

    #[test]
    fn test_transfer_admin_unauthorized_fails() {
        let (env, client, _, _) = setup();
        let admin = Address::generate(&env);
        let attacker = Address::generate(&env);
        let new_admin = Address::generate(&env);
        client.initialize(&admin);
        let result = client.try_transfer_admin(&attacker, &new_admin);
        assert_eq!(result, Err(Ok(QuoteError::Unauthorized)));
    }

    #[test]
    fn test_transfer_admin_emits_event() {
        let (env, client, _, _) = setup();
        let admin = Address::generate(&env);
        let new_admin = Address::generate(&env);
        client.initialize(&admin);
        client.transfer_admin(&admin, &new_admin);
        let event = env.events().all().last().unwrap().clone();
        let topic: soroban_sdk::Symbol = event.1.get(0).unwrap().into_val(&env);
        assert_eq!(topic, soroban_sdk::Symbol::new(&env, "admin_transferred"));
    }

    #[test]
    fn test_transfer_admin_not_initialized_fails() {
        let (env, client, _, _) = setup();
        let current = Address::generate(&env);
        let new_admin = Address::generate(&env);
        let result = client.try_transfer_admin(&current, &new_admin);
        assert_eq!(result, Err(Ok(QuoteError::NotInitialized)));
    }

    // ── Single-hop tests ──────────────────────────────────────────────────────

    #[test]
    fn test_single_hop_exchange_rate() {
        let (env, client, double, _) = setup();
        let ti = Address::generate(&env);
        let to = Address::generate(&env);
        let resp = client.get_quote(&double, &ti, &to, &1_000_000, &0, &0, &6);
        assert_eq!(resp.amount_out, 2_000_000);
        assert_eq!(resp.exchange_rate, 2_000_000);
    }

    #[test]
    fn test_single_hop_fee_deducted() {
        let (env, client, double, _) = setup();
        let ti = Address::generate(&env);
        let to = Address::generate(&env);
        let resp = client.get_quote(&double, &ti, &to, &1_000_000, &30, &0, &6);
        assert_eq!(resp.total_fee_amount, 3_000);
    }

    #[test]
    fn test_single_hop_slippage() {
        let (env, client, double, _) = setup();
        let ti = Address::generate(&env);
        let to = Address::generate(&env);
        let resp = client.get_quote(&double, &ti, &to, &1_000_000, &0, &50, &6);
        assert_eq!(resp.min_amount_out, 1_990_000);
    }

    #[test]
    fn test_single_hop_invalid_amount() {
        let (env, client, double, _) = setup();
        let ti = Address::generate(&env);
        let to = Address::generate(&env);
        let r = client.try_get_quote(&double, &ti, &to, &0, &0, &0, &6);
        assert_eq!(r, Err(Ok(QuoteError::InvalidAmount)));
    }

    #[test]
    fn test_single_hop_invalid_precision() {
        let (env, client, double, _) = setup();
        let ti = Address::generate(&env);
        let to = Address::generate(&env);
        let r = client.try_get_quote(&double, &ti, &to, &1_000_000, &0, &0, &0);
        assert_eq!(r, Err(Ok(QuoteError::InvalidPrecision)));
    }

    #[test]
    fn test_single_hop_invalid_slippage() {
        let (env, client, double, _) = setup();
        let ti = Address::generate(&env);
        let to = Address::generate(&env);
        let r = client.try_get_quote(&double, &ti, &to, &1_000_000, &0, &10_001, &6);
        assert_eq!(r, Err(Ok(QuoteError::InvalidSlippage)));
    }

    // ── Multi-hop tests ───────────────────────────────────────────────────────

    #[test]
    fn test_multihop_two_hops_chains_correctly() {
        let (env, client, double, triple) = setup();
        let ta = Address::generate(&env);
        let tb = Address::generate(&env);
        let tc = Address::generate(&env);
        let mut hops = soroban_sdk::Vec::new(&env);
        hops.push_back(HopDescriptor { plugin: double, token_in: ta, token_out: tb.clone(), fee_bps: 0 });
        hops.push_back(HopDescriptor { plugin: triple, token_in: tb, token_out: tc, fee_bps: 0 });
        let resp = client.get_multihop_quote(&hops, &100, &0, &6);
        assert_eq!(resp.amount_out, 600);
        assert_eq!(resp.hops.get(0).unwrap().amount_out, 200);
        assert_eq!(resp.hops.get(1).unwrap().amount_out, 600);
    }

    #[test]
    fn test_multihop_exchange_rate_end_to_end() {
        let (env, client, double, triple) = setup();
        let ta = Address::generate(&env);
        let tb = Address::generate(&env);
        let tc = Address::generate(&env);
        let mut hops = soroban_sdk::Vec::new(&env);
        hops.push_back(HopDescriptor { plugin: double, token_in: ta, token_out: tb.clone(), fee_bps: 0 });
        hops.push_back(HopDescriptor { plugin: triple, token_in: tb, token_out: tc, fee_bps: 0 });
        let resp = client.get_multihop_quote(&hops, &100, &0, &2);
        assert_eq!(resp.exchange_rate, 600);
    }

    #[test]
    fn test_multihop_fees_accumulated_per_hop() {
        let (env, client, double, triple) = setup();
        let ta = Address::generate(&env);
        let tb = Address::generate(&env);
        let tc = Address::generate(&env);
        let mut hops = soroban_sdk::Vec::new(&env);
        hops.push_back(HopDescriptor { plugin: double, token_in: ta, token_out: tb.clone(), fee_bps: 100 });
        hops.push_back(HopDescriptor { plugin: triple, token_in: tb, token_out: tc, fee_bps: 200 });
        let resp = client.get_multihop_quote(&hops, &1000, &0, &6);
        assert_eq!(resp.total_fee_amount, 50);
    }

    #[test]
    fn test_multihop_empty_route_fails() {
        let (env, client, _, _) = setup();
        let hops: soroban_sdk::Vec<HopDescriptor> = soroban_sdk::Vec::new(&env);
        let r = client.try_get_multihop_quote(&hops, &1000, &0, &6);
        assert_eq!(r, Err(Ok(QuoteError::EmptyRoute)));
    }

    #[test]
    fn test_multihop_token_mismatch_fails() {
        let (env, client, double, triple) = setup();
        let ta = Address::generate(&env);
        let tb = Address::generate(&env);
        let tc = Address::generate(&env);
        let td = Address::generate(&env); // unrelated token — breaks chain

        // Hop 1: A → B, Hop 2: C → D (B != C → TokenMismatch)
        let mut hops = soroban_sdk::Vec::new(&env);
        hops.push_back(HopDescriptor { plugin: double, token_in: ta, token_out: tb, fee_bps: 0 });
        hops.push_back(HopDescriptor { plugin: triple, token_in: tc, token_out: td, fee_bps: 0 });

        let r = client.try_get_multihop_quote(&hops, &1000, &0, &6);
        assert_eq!(r, Err(Ok(QuoteError::TokenMismatch)));
    }    #[test]
    fn test_multihop_too_many_hops_fails() {
        let (env, client, double, _) = setup();
        let ta = Address::generate(&env);
        let tb = Address::generate(&env);
        let mut hops = soroban_sdk::Vec::new(&env);
        for _ in 0..6 {
            hops.push_back(HopDescriptor { plugin: double.clone(), token_in: ta.clone(), token_out: tb.clone(), fee_bps: 0 });
        }
        let r = client.try_get_multihop_quote(&hops, &1000, &0, &6);
        assert_eq!(r, Err(Ok(QuoteError::RouteTooLong)));
    }

    // ── Fee estimate tests ────────────────────────────────────────────────────

    #[test]
    fn test_estimate_fee_normal_load() {
        let (_, client, _, _) = setup();
        let req = FeeEstimateRequest { amount: 1_000_000, fee_bps: 30, network_load_bps: 5_000 };
        let resp = client.estimate_fee(&req);
        assert!(!resp.surge_pricing);
        assert_eq!(resp.protocol_fee, 3_000);
        assert_eq!(resp.network_fee, 100);
        assert_eq!(resp.total_fee, 3_100);
        assert_eq!(resp.effective_fee_bps, 30);
    }

    #[test]
    fn test_estimate_fee_surge_pricing() {
        let (_, client, _, _) = setup();
        let req = FeeEstimateRequest { amount: 1_000_000, fee_bps: 30, network_load_bps: 9_000 };
        let resp = client.estimate_fee(&req);
        assert!(resp.surge_pricing);
        assert_eq!(resp.network_fee, 200);
        assert_eq!(resp.effective_fee_bps, 60);
    }

    #[test]
    fn test_estimate_fee_invalid_amount() {
        let (_, client, _, _) = setup();
        let req = FeeEstimateRequest { amount: 0, fee_bps: 30, network_load_bps: 0 };
        let r = client.try_estimate_fee(&req);
        assert_eq!(r, Err(Ok(QuoteError::InvalidAmount)));
    }

    #[test]
    fn test_estimate_fees_skips_invalid() {
        let (env, client, _, _) = setup();
        let requests = soroban_sdk::vec![
            &env,
            FeeEstimateRequest { amount: 1_000_000, fee_bps: 30, network_load_bps: 0 },
            FeeEstimateRequest { amount: 0, fee_bps: 30, network_load_bps: 0 },
            FeeEstimateRequest { amount: 500_000, fee_bps: 10, network_load_bps: 0 },
        ];
        let responses = client.estimate_fees(&requests);
        assert_eq!(responses.len(), 2);
    }
}

    #[test]
    fn test_compare_quotes_selects_highest_amount_out() {
        let (env, client, double, triple) = setup();
        let ti = Address::generate(&env);
        let to = Address::generate(&env);

        let mut routes = soroban_sdk::Vec::new(&env);
        routes.push_back(client.get_quote(&double, &ti, &to, &1000, &0, &0, &6));
        routes.push_back(client.get_quote(&triple, &ti, &to, &1000, &0, &0, &6));

        // triple gives amount_out = 3000, double gives 2000 → triple wins
        let best = client.compare_quotes(&routes, &10_000u32);
        assert_eq!(best.amount_out, 3000);
    }

    #[test]
    fn test_compare_quotes_filters_by_price_impact() {
        let (env, client, double, triple) = setup();
        let ti = Address::generate(&env);
        let to = Address::generate(&env);

        // Both plugins return more than amount_in so price_impact_bps > 0 (positive).
        // Use a threshold of 0 bps — routes with price_impact_bps < 0 are excluded.
        // Since both plugins return > amount_in, neither is filtered.
        let mut routes = soroban_sdk::Vec::new(&env);
        routes.push_back(client.get_quote(&double, &ti, &to, &1000, &0, &0, &6));
        routes.push_back(client.get_quote(&triple, &ti, &to, &1000, &0, &0, &6));

        let best = client.compare_quotes(&routes, &0u32);
        assert_eq!(best.amount_out, 3000);
        let mut route1 = soroban_sdk::Vec::new(&env);
        route1.push_back(HopDescriptor { plugin: double, token_in: ti.clone(), token_out: to.clone(), fee_bps: 0 });

        let mut route2 = soroban_sdk::Vec::new(&env);
        route2.push_back(HopDescriptor { plugin: triple, token_in: ti.clone(), token_out: to.clone(), fee_bps: 0 });

        let mut routes = soroban_sdk::Vec::new(&env);
        routes.push_back(route1);
        routes.push_back(route2);

        let best = client.get_best_quote(&routes, &1000, &0, &6);
        assert_eq!(best.amount_out, 3000); // triple plugin wins
    }

    #[test]
    fn test_compare_quotes_all_filtered_returns_error() {
        let (env, client, _, _) = setup();

        // Construct a quote with a very negative price impact manually.
        let bad_quote = QuoteResponse {
            amount_out: 1,
            total_fee_amount: 0,
            min_amount_out: 1,
            exchange_rate: 1,
            precision: 6,
            price_impact_bps: -5000, // -50% impact
            hops: soroban_sdk::Vec::new(&env),
        };

        let mut routes = soroban_sdk::Vec::new(&env);
        routes.push_back(bad_quote);

        // Threshold of 100 bps (1%) — the -50% route is filtered out
        let result = client.try_compare_quotes(&routes, &100u32);
        assert_eq!(result, Err(Ok(QuoteError::QuoteFailed)));
    }

    #[test]
    fn test_compare_quotes_empty_routes_returns_error() {
        let (env, client, _, _) = setup();
        let routes: soroban_sdk::Vec<QuoteResponse> = soroban_sdk::Vec::new(&env);
        let result = client.try_compare_quotes(&routes, &100u32);
        assert_eq!(result, Err(Ok(QuoteError::EmptyRoute)));
    }

    #[test]
    fn test_compare_quotes_single_route_within_threshold() {
        let (env, client, double, _) = setup();
        let ti = Address::generate(&env);
        let to = Address::generate(&env);

        let mut routes = soroban_sdk::Vec::new(&env);
        routes.push_back(client.get_quote(&double, &ti, &to, &1000, &0, &0, &6));

        let best = client.compare_quotes(&routes, &10_000u32);
        assert_eq!(best.amount_out, 2000);
        let mut route = soroban_sdk::Vec::new(&env);
        route.push_back(HopDescriptor { plugin: double, token_in: ti.clone(), token_out: to.clone(), fee_bps: 0 });

        let mut routes = soroban_sdk::Vec::new(&env);
        routes.push_back(route);

        let best = client.get_best_quote(&routes, &1000, &0, &6);
        assert_eq!(best.amount_out, 2000);
    }

    #[test]
    fn test_get_best_quote_empty_list_fails() {
        let (env, client, _, _) = setup();
        let routes: soroban_sdk::Vec<soroban_sdk::Vec<HopDescriptor>> = soroban_sdk::Vec::new(&env);
        let result = client.try_get_best_quote(&routes, &1000, &0, &6);
        assert_eq!(result, Err(Ok(QuoteError::EmptyRoute)));
    }
}

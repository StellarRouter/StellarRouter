#![no_std]

//! # router-quote
//!
//! Preview transaction results before execution. Supports single-hop and
//! multi-hop quotes where the output of one pool feeds into the next.
//!
//! ## Multi-hop routing
//!
//! A multi-hop quote chains N liquidity plugin calls:
//!
//!   token_A → [plugin_1] → token_B → [plugin_2] → token_C
//!
//! Each plugin must implement `get_quote(token_in, token_out, amount_in) -> i128`.
//! The `amount_out` of hop N becomes the `amount_in` of hop N+1.
//! Fees and slippage are applied at each hop independently.
//!
//! ## Exchange rate
//!
//! Exchange rates are fixed-point integers with configurable decimal precision:
//!
//!   exchange_rate = (amount_out * 10^precision) / amount_in
//!
//! A rate of `2_000_000` with `precision = 6` means 2.000000 token_out per token_in.
//! Preview transaction results before execution.
//! Returns expected output amount, fees, exchange rate, and route details
//! without executing the transaction.
//!
//! ## Exchange Rate
//!
//! Exchange rates are calculated as a fixed-point integer with configurable
//! decimal precision. For example, with `precision = 6`:
//!
//!   exchange_rate = (amount_out * 10^precision) / amount_in
//!
//! A rate of `1_050_000` with precision 6 means 1.050000 token_out per token_in.
//! This avoids floating point entirely and is safe for on-chain use.

extern crate alloc;
use alloc::string::ToString;
//! Allows users to get quote information including expected output amount,
//! fees, and route details without executing the transaction.
//!
//! ## Features
//! - Get quote from any registered liquidity plugin
//! - Returns expected output amount, fees, and route details
//! - Does not execute transactions (read-only preview)
//! - Works with any plugin implementing the get_quote interface
//!
//! ## Events (following naming convention: past tense verbs in snake_case)
//! - `quote_requested` — Quote request logged (route_name, token_in, token_out, amount_in)

use soroban_sdk::{
    contract, contracterror, contractimpl, contracttype, Address, Env, String, Symbol, Vec,
};

// ── Types ─────────────────────────────────────────────────────────────────────

/// A single hop in a multi-hop route.
#[contracttype]
#[derive(Clone, Debug, PartialEq)]
pub struct HopDescriptor {
    /// Liquidity plugin contract address for this hop.
    pub plugin: Address,
    /// Token being sold in this hop.
    pub token_in: Address,
    /// Token being received in this hop.
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
/// Request parameters for getting a quote.
#[contracttype]
#[derive(Clone, Debug)]
pub struct QuoteRequest {
    /// The route name to query (e.g., "liquidity/uniswap-v3")
    pub route_name: String,
    /// The token the user is selling
    pub token_in: Address,
    /// The token the user wants to receive
    pub token_out: Address,
    /// The amount of token_in to swap
    pub amount_in: i128,
}

/// Response containing quote details.
#[contracttype]
#[derive(Clone, Debug, PartialEq)]
pub struct QuoteResponse {
    /// The expected output amount (token_out units).
    pub amount_out: i128,
    /// The fee amount deducted (in token_in units).
    pub fee_amount: i128,
    /// The route name that was used.
    pub route_name: String,
    /// The target contract address.
    pub target: Address,
    /// Minimum output amount after slippage tolerance.
    pub min_amount_out: i128,
    /// Exchange rate as a fixed-point integer.
    /// Value = (amount_out * 10^precision) / amount_in.
    /// Use `precision` field to interpret.
    pub exchange_rate: i128,
    /// Number of decimal places in `exchange_rate`.
    pub precision: u32,
    /// Price impact in basis points (negative = adverse).
    pub price_impact_bps: i32,
}

/// Fee estimate breakdown.
#[contracttype]
#[derive(Clone, Debug, PartialEq)]
pub struct FeeEstimateResponse {
    pub protocol_fee: i128,
    pub network_fee: i128,
    pub total_fee: i128,
    pub surge_pricing: bool,
    /// Protocol fee in token_in base units.
    pub protocol_fee: i128,
    /// Network fee in stroops.
    pub network_fee: i128,
    /// Total fee (protocol + network).
    pub total_fee: i128,
    /// Whether surge pricing was applied.
    /// The expected output amount (token_out)
    pub amount_out: i128,
    /// The fee amount deducted (in token_in)
    pub fee_amount: i128,
    /// The route name that was used
    pub route_name: String,
    /// The target contract address
    pub target: Address,
    /// Minimum output amount (for slippage protection)
    pub min_amount_out: i128,
    /// Exchange rate (amount_out / amount_in as a string for precision)
    pub exchange_rate: String,
    /// Price impact estimate (basis points, negative = adverse)
    pub price_impact_bps: i32,
}

// ── Errors ────────────────────────────────────────────────────────────────────

#[contracterror]
#[derive(Copy, Clone, Debug, PartialEq)]
pub enum QuoteError {
    RouteNotFound = 1,
    InvalidAmount = 2,
    QuoteFailed = 3,
    InvalidRoute = 4,
}

/// Request parameters for fee estimation.
#[contracttype]
#[derive(Clone, Debug, PartialEq)]
pub struct FeeEstimateRequest {
    /// Amount of token_in being transacted (in stroops or token base units).
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
    InvalidAmount = 1,
    RouteNotFound = 2,
    QuoteFailed = 3,
    InvalidPrecision = 4,
    InvalidSlippage = 5,
    EmptyRoute = 6,
    RouteTooLong = 7,
}

// Maximum hops allowed in a multi-hop route. Keeps gas costs bounded.
const MAX_HOPS: u32 = 5;

}

// ── Contract ──────────────────────────────────────────────────────────────────

#[contract]
pub struct RouterQuote;

#[contractimpl]
impl RouterQuote {
    /// Get a single-hop quote from a liquidity plugin.
    ///
    /// Calls `get_quote(token_in, token_out, amount_in) -> i128` on `plugin`
    /// and returns a full [`QuoteResponse`] with exchange rate, slippage-adjusted
    /// minimum output, and fee breakdown.
    ///
    /// # Arguments
    /// * `env` - The Soroban environment.
    /// * `plugin` - Liquidity plugin contract address.
    /// * `token_in` - Token being sold.
    /// * `token_out` - Token being bought.
    /// * `amount_in` - Amount of token_in (must be > 0).
    /// * `fee_bps` - Protocol fee in basis points.
    /// * `slippage_bps` - Slippage tolerance in basis points (0–10000).
    /// * `precision` - Decimal places for exchange rate (1–18).
    ///
    /// # Errors
    /// * [`QuoteError::InvalidAmount`] — `amount_in` ≤ 0.
    /// * [`QuoteError::InvalidPrecision`] — `precision` is 0 or > 18.
    /// * [`QuoteError::InvalidSlippage`] — `slippage_bps` > 10000.
    /// * [`QuoteError::QuoteFailed`] — plugin call failed.
    pub fn get_quote(
        env: Env,
        plugin: Address,
    /// Get a quote from a liquidity plugin contract.
    ///
    /// Calls `get_quote(token_in, token_out, amount_in) -> i128` on `target`
    /// and returns a full [`QuoteResponse`] including a properly calculated
    /// exchange rate.
    ///
    /// # Arguments
    /// * `env` - The Soroban environment.
    /// * `target` - The liquidity plugin contract address.
    /// * `route_name` - Human-readable name for the route.
    /// * `token_in` - Token being sold.
    /// * `token_out` - Token being bought.
    /// * `amount_in` - Amount of token_in to swap (must be > 0).
    /// * `fee_bps` - Protocol fee in basis points (e.g. 30 = 0.30%).
    /// * `slippage_bps` - Slippage tolerance in basis points (e.g. 50 = 0.50%).
    /// * `precision` - Decimal places for the exchange rate (1–18, typically 6).
    ///
    /// # Returns
    /// A [`QuoteResponse`] with `exchange_rate` = `(amount_out * 10^precision) / amount_in`.
    ///
    /// # Errors
    /// * [`QuoteError::InvalidAmount`] — if `amount_in` ≤ 0.
    /// * [`QuoteError::InvalidPrecision`] — if `precision` is 0 or > 18.
    /// * [`QuoteError::InvalidSlippage`] — if `slippage_bps` > 10000.
    /// * [`QuoteError::QuoteFailed`] — if the plugin call fails.
    pub fn get_quote(
        env: Env,
        target: Address,
    /// Get a quote from a liquidity plugin.
    ///
    /// Resolves the route name to a contract address via router-core (if provided),
    /// then invokes the plugin's `get_quote` function to retrieve the expected output.
    /// This does NOT execute the transaction — it only previews the result.
    ///
    /// # Arguments
    /// * `env` - The Soroban environment.
    /// * `router_core` - Optional address of router-core contract for route resolution.
    /// * `route_name` - The name of the route to query (e.g., "liquidity/uniswap-v3").
    ///                   Can also be a direct contract address if router_core is None.
    /// * `token_in` - The address of the token being sold.
    /// * `token_out` - The address of the token being bought.
    /// * `amount_in` - The amount of token_in to swap.
    ///
    /// # Returns
    /// A [`QuoteResponse`] containing the expected output amount, fees, and route details.
    ///
    /// # Errors
    /// * [`QuoteError::InvalidAmount`] — if `amount_in` is less than or equal to zero.
    /// * [`QuoteError::RouteNotFound`] — if the route name is not registered.
    /// * [`QuoteError::QuoteFailed`] — if the plugin's `get_quote` call fails.
    pub fn get_quote(
        env: Env,
        router_core: Option<Address>,
        route_name: String,
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

        let hop = HopDescriptor {
            plugin,
            token_in,
            token_out,
            fee_bps,
        };
        let mut hops = Vec::new(&env);
        hops.push_back(hop);

        Self::execute_hops(&env, hops, amount_in, slippage_bps, precision)
    }

    /// Get a multi-hop quote chaining N liquidity plugins.
    ///
    /// Executes hops in order: the `amount_out` of hop N becomes the
    /// `amount_in` of hop N+1. The final `QuoteResponse` reflects the
    /// end-to-end exchange rate and total fees across all hops.
    ///
    /// # Arguments
    /// * `env` - The Soroban environment.
    /// * `hops` - Ordered list of [`HopDescriptor`]s (1–5 hops).
    /// * `amount_in` - Initial input amount (must be > 0).
    /// * `slippage_bps` - Slippage tolerance applied to the final output (0–10000).
    /// * `precision` - Decimal places for the end-to-end exchange rate (1–18).
    ///
    /// # Errors
    /// * [`QuoteError::EmptyRoute`] — `hops` is empty.
    /// * [`QuoteError::RouteTooLong`] — `hops` has more than `MAX_HOPS` entries.
    /// * [`QuoteError::InvalidAmount`] — `amount_in` ≤ 0.
    /// * [`QuoteError::InvalidPrecision`] — `precision` is 0 or > 18.
    /// * [`QuoteError::InvalidSlippage`] — `slippage_bps` > 10000.
    /// * [`QuoteError::QuoteFailed`] — any plugin call failed.
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

        Self::execute_hops(&env, hops, amount_in, slippage_bps, precision)
        // Call the plugin's get_quote function
        let function = Symbol::new(&env, "get_quote");
    ) -> Result<QuoteResponse, QuoteError> {
        // Validate input
        if amount_in <= 0 {
            return Err(QuoteError::InvalidAmount);
        }

        // Resolve target address
        let target: Address = match router_core {
            Some(router) => {
                // Use router-core to resolve the route name
                let function = Symbol::new(&env, "resolve");
                let mut args = Vec::new(&env);
                args.push_back(route_name.clone().into());
                
                env.invoke_contract(&router, &function, args)
            }
            None => {
                // Try direct address interpretation
                route_name.clone().try_into().map_err(|_| QuoteError::InvalidRoute)?
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
        
        // Calculate min_amount_out with 0.5% slippage tolerance
        let min_amount_out = amount_out * 999 / 1000;
        
        // Exchange rate placeholder
        let exchange_rate = String::from_str(&env, "0");

        // Price impact (0 for now - would need more complex calculation)
        let price_impact_bps = 0;

        Ok(QuoteResponse {
            amount_out,
            fee_amount,
            route_name,
            target,
            min_amount_out,
            exchange_rate,
            precision,
            price_impact_bps,
        })
    }

    /// Estimate fees for a transaction.
    ///
    /// # Arguments
    /// * `env` - The Soroban environment.
    /// * `amount` - Transaction amount (must be > 0).
    /// * `fee_bps` - Route fee rate in basis points.
    /// * `network_load_bps` - Network utilization (0–10000). ≥ 8000 triggers 2× surge.
    ///
    /// # Errors
    /// * [`QuoteError::InvalidAmount`] — `amount` ≤ 0.
    /// * `amount` - Transaction amount in token base units (must be > 0).
    /// * `fee_bps` - Route fee rate in basis points.
    /// * `network_load_bps` - Current network utilization (0–10000).
    ///   Values ≥ 8000 trigger 2× surge pricing on the network fee.
    ///
    /// # Errors
    /// * [`QuoteError::InvalidAmount`] — if `amount` ≤ 0.
    pub fn estimate_fee(
        env: Env,
        amount: i128,
        fee_bps: u32,
        network_load_bps: u32,
    ) -> Result<FeeEstimateResponse, QuoteError> {
        if amount <= 0 {
            return Err(QuoteError::InvalidAmount);
        }

        let protocol_fee = amount * fee_bps as i128 / 10_000;
        let base_network_fee: i128 = 100;
        let base_network_fee: i128 = 100; // 100 stroops minimum

        let (network_fee, surge_pricing, effective_fee_bps) = if network_load_bps >= 8_000 {
            (base_network_fee * 2, true, fee_bps * 2)
        } else {
            (base_network_fee, false, fee_bps)
        };

        env.events().publish(
            (Symbol::new(&env, "fee_estimated"),),
            (amount, fee_bps, surge_pricing),
        );

        Ok(FeeEstimateResponse {
            protocol_fee,
            network_fee,
            total_fee: protocol_fee + network_fee,
            surge_pricing,
            effective_fee_bps,
        })
    }

    // ── Helpers ───────────────────────────────────────────────────────────────

    /// Execute a chain of hops and aggregate results.
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
            let gross_amount_out = Self::call_plugin(env, &hop.plugin, &hop.token_in, &hop.token_out, current_amount)?;

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

        // Slippage applied to the final output only
        let min_amount_out = final_amount_out * (10_000 - slippage_bps as i128) / 10_000;

        // End-to-end exchange rate: (final_out * 10^precision) / initial_in
        let scale = Self::pow10(precision);
        let exchange_rate = (final_amount_out * scale) / initial_amount_in;

        // Price impact: (final_out - initial_in) * 10_000 / initial_in
        let price_impact_bps = ((final_amount_out - initial_amount_in) * 10_000 / initial_amount_in) as i32;

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

    /// Call a liquidity plugin's `get_quote` function.
    fn call_plugin(
        env: &Env,
        plugin: &Address,
        token_in: &Address,
        token_out: &Address,
        amount_in: i128,
    ) -> Result<i128, QuoteError> {
        let function = Symbol::new(env, "get_quote");
        let mut args = Vec::new(env);
        args.push_back(token_in.clone().into());
        args.push_back(token_out.clone().into());
        args.push_back(amount_in.into());

        env.try_invoke_contract::<i128, i128>(plugin, &function, args)
            .map_err(|_| QuoteError::QuoteFailed)?
            .map_err(|_| QuoteError::QuoteFailed)
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
    ) -> Vec<QuoteResponse> {
        let mut responses = Vec::new(&env);
        
        for req in requests.iter() {
            let response = Self::get_quote(
                env.clone(),
                router_core.clone(),
                req.route_name.clone(),
                req.token_in.clone(),
                req.token_out.clone(),
                req.amount_in,
            );
            
            match response {
                Ok(quote) => responses.push_back(quote),
                Err(_) => {
                    // On failure, add a zero quote (caller can check amount_out == 0)
                    responses.push_back(QuoteResponse {
                        amount_out: 0,
                        fee_amount: 0,
                        route_name: req.route_name.clone(),
                        target: req.route_name.clone().try_into().unwrap_or(Address::from_contract_id(&env, &[0u8; 32])),
                        min_amount_out: 0,
                        exchange_rate: String::from_str(&env, "0"),
                        price_impact_bps: 0,
                    });
                }
            }
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

    // ── Helpers ───────────────────────────────────────────────────────────────

    /// Returns 10^exp as i128. Safe for exp ≤ 18.
    fn pow10(exp: u32) -> i128 {
        let mut result: i128 = 1;
        let mut i = 0u32;
        while i < exp {
            result *= 10;
            i += 1;
        }
        result
    /// Estimate fees for multiple transactions in one call.
    ///
    /// Processes each [`FeeEstimateRequest`] independently. Failed estimates
    /// (e.g., invalid amount) are skipped and not included in the result.
    ///
    /// # Arguments
    /// * `env` - The Soroban environment.
    /// * `requests` - A vector of [`FeeEstimateRequest`]s.
    ///
    /// # Returns
    /// A vector of [`FeeEstimateResponse`]s for each valid request.
    pub fn estimate_fees(env: Env, requests: Vec<FeeEstimateRequest>) -> Vec<FeeEstimateResponse> {
        let mut responses = Vec::new(&env);
        for req in requests.iter() {
            if let Ok(estimate) = Self::estimate_fee(env.clone(), req) {
                responses.push_back(estimate);
            }
        }
        responses
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    extern crate std;
    use super::*;
    use soroban_sdk::{testutils::Address as _, Env};

    /// Mock plugin: returns amount_in * 2
    #[soroban_sdk::contract]
    pub struct DoublePlugin;

    #[soroban_sdk::contractimpl]
    impl DoublePlugin {
        pub fn get_quote(_env: Env, _ti: Address, _to: Address, amount_in: i128) -> i128 {
    use soroban_sdk::{testutils::Address as _, Env, Symbol};

    // A minimal mock liquidity plugin that returns amount_in * 2
    #[soroban_sdk::contract]
    pub struct MockPlugin;

    #[soroban_sdk::contractimpl]
    impl MockPlugin {
        pub fn get_quote(_env: Env, _token_in: Address, _token_out: Address, amount_in: i128) -> i128 {
            amount_in * 2
        }
    }

    /// Mock plugin: returns amount_in * 3
    #[soroban_sdk::contract]
    pub struct TriplePlugin;

    #[soroban_sdk::contractimpl]
    impl TriplePlugin {
        pub fn get_quote(_env: Env, _ti: Address, _to: Address, amount_in: i128) -> i128 {
            amount_in * 3
        }
    }

    fn setup() -> (Env, RouterQuoteClient<'static>, Address, Address) {
        let env = Env::default();
        env.mock_all_auths();
        let id = env.register_contract(None, RouterQuote);
        let client = RouterQuoteClient::new(&env, &id);
        let double = env.register_contract(None, DoublePlugin);
        let triple = env.register_contract(None, TriplePlugin);
        (env, client, double, triple)
    }

    // ── Single-hop tests ──────────────────────────────────────────────────────

    #[test]
    fn test_single_hop_exchange_rate() {
        let (env, client, double, _) = setup();
        let ti = Address::generate(&env);
        let to = Address::generate(&env);
        // amount_in=1_000_000, plugin returns 2_000_000
        // rate = (2_000_000 * 10^6) / 1_000_000 = 2_000_000
        let resp = client.get_quote(&double, &ti, &to, &1_000_000, &0, &0, &6);
        assert_eq!(resp.amount_out, 2_000_000);
        assert_eq!(resp.exchange_rate, 2_000_000);
    fn setup() -> (Env, RouterQuoteClient<'static>, Address) {
    use super::*;
    use soroban_sdk::Env;

    fn setup() -> (Env, RouterQuoteClient<'static>) {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register_contract(None, RouterQuote);
        let client = RouterQuoteClient::new(&env, &contract_id);
        let plugin_id = env.register_contract(None, MockPlugin);
        (env, client, plugin_id)
    }

    #[test]
    fn test_exchange_rate_calculated_correctly() {
        let (env, client, plugin) = setup();
        let token_in = Address::generate(&env);
        let token_out = Address::generate(&env);
        let route = String::from_str(&env, "mock/pool");

        // amount_in = 1_000_000, plugin returns 2_000_000
        // exchange_rate = (2_000_000 * 10^6) / 1_000_000 = 2_000_000
        let resp = client.get_quote(
            &plugin, &route, &token_in, &token_out,
            &1_000_000, &0, &0, &6,
        );
        assert_eq!(resp.amount_out, 2_000_000);
        assert_eq!(resp.exchange_rate, 2_000_000); // 2.000000 with precision 6
        assert_eq!(resp.precision, 6);
    }

    #[test]
    fn test_single_hop_fee_deducted() {
        let (env, client, double, _) = setup();
        let ti = Address::generate(&env);
        let to = Address::generate(&env);
        // fee_bps=30 (0.30%), amount_in=1_000_000 → fee=3_000
        let resp = client.get_quote(&double, &ti, &to, &1_000_000, &30, &0, &6);
        assert_eq!(resp.total_fee_amount, 3_000);
    }

    #[test]
    fn test_single_hop_slippage() {
        let (env, client, double, _) = setup();
        let ti = Address::generate(&env);
        let to = Address::generate(&env);
        // slippage_bps=50, amount_out=2_000_000
        // min = 2_000_000 * 9950 / 10_000 = 1_990_000
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

        // Hop 1: A → B via double (×2), Hop 2: B → C via triple (×3)
        // amount_in=100 → hop1_out=200 → hop2_out=600
        let mut hops = soroban_sdk::Vec::new(&env);
        hops.push_back(HopDescriptor { plugin: double, token_in: ta, token_out: tb.clone(), fee_bps: 0 });
        hops.push_back(HopDescriptor { plugin: triple, token_in: tb, token_out: tc, fee_bps: 0 });

        let resp = client.get_multihop_quote(&hops, &100, &0, &6);
        assert_eq!(resp.amount_out, 600);
        assert_eq!(resp.hops.len(), 2);
        assert_eq!(resp.hops.get(0).unwrap().amount_out, 200);
        assert_eq!(resp.hops.get(1).unwrap().amount_out, 600);
    }

    #[test]
    fn test_multihop_exchange_rate_end_to_end() {
        let (env, client, double, triple) = setup();
        let ta = Address::generate(&env);
        let tb = Address::generate(&env);
        let tc = Address::generate(&env);

        // amount_in=100, final_out=600
        // rate = (600 * 10^2) / 100 = 600
        let mut hops = soroban_sdk::Vec::new(&env);
        hops.push_back(HopDescriptor { plugin: double.clone(), token_in: ta.clone(), token_out: tb.clone(), fee_bps: 0 });
        hops.push_back(HopDescriptor { plugin: triple.clone(), token_in: tb.clone(), token_out: tc.clone(), fee_bps: 0 });

        let resp = client.get_multihop_quote(&hops, &100, &0, &2);
        assert_eq!(resp.exchange_rate, 600); // 6.00 with precision 2
    }

    #[test]
    fn test_multihop_fees_accumulated_per_hop() {
        let (env, client, double, triple) = setup();
        let ta = Address::generate(&env);
        let tb = Address::generate(&env);
        let tc = Address::generate(&env);

        // Hop 1: fee_bps=100 (1%), amount_in=1000 → fee=10
        // Hop 2: fee_bps=200 (2%), amount_in=2000 → fee=40
        // total_fee = 50
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
    fn test_multihop_too_many_hops_fails() {
        let (env, client, double, _) = setup();
        let ta = Address::generate(&env);
        let tb = Address::generate(&env);
        let mut hops = soroban_sdk::Vec::new(&env);
        // Push 6 hops (MAX_HOPS = 5)
        for _ in 0..6 {
            hops.push_back(HopDescriptor {
                plugin: double.clone(),
                token_in: ta.clone(),
                token_out: tb.clone(),
                fee_bps: 0,
            });
        }
        let r = client.try_get_multihop_quote(&hops, &1000, &0, &6);
        assert_eq!(r, Err(Ok(QuoteError::RouteTooLong)));
    }

    // ── Fee estimate tests ────────────────────────────────────────────────────

    #[test]
    fn test_estimate_fee_normal_load() {
        let (_, client, _, _) = setup();
    fn test_exchange_rate_precision_2() {
        let (env, client, plugin) = setup();
        let token_in = Address::generate(&env);
        let token_out = Address::generate(&env);
        let route = String::from_str(&env, "mock/pool");

        // amount_in = 100, plugin returns 200
        // exchange_rate = (200 * 10^2) / 100 = 200 → 2.00 with precision 2
        let resp = client.get_quote(
            &plugin, &route, &token_in, &token_out,
            &100, &0, &0, &2,
        );
        assert_eq!(resp.exchange_rate, 200);
        assert_eq!(resp.precision, 2);
    }

    #[test]
    fn test_fee_deducted_correctly() {
        let (env, client, plugin) = setup();
        let token_in = Address::generate(&env);
        let token_out = Address::generate(&env);
        let route = String::from_str(&env, "mock/pool");

        // fee_bps = 30 (0.30%), amount_in = 1_000_000
        // fee = 1_000_000 * 30 / 10_000 = 3_000
        let resp = client.get_quote(
            &plugin, &route, &token_in, &token_out,
            &1_000_000, &30, &0, &6,
        );
        assert_eq!(resp.fee_amount, 3_000);
    }

    #[test]
    fn test_slippage_applied_to_min_amount_out() {
        let (env, client, plugin) = setup();
        let token_in = Address::generate(&env);
        let token_out = Address::generate(&env);
        let route = String::from_str(&env, "mock/pool");

        // slippage_bps = 50 (0.50%), amount_out = 2_000_000
        // min_amount_out = 2_000_000 * 9950 / 10_000 = 1_990_000
        let resp = client.get_quote(
            &plugin, &route, &token_in, &token_out,
            &1_000_000, &0, &50, &6,
        );
        assert_eq!(resp.min_amount_out, 1_990_000);
    }

    #[test]
    fn test_invalid_amount_fails() {
        let (env, client, plugin) = setup();
        let token_in = Address::generate(&env);
        let token_out = Address::generate(&env);
        let route = String::from_str(&env, "mock/pool");
        let result = client.try_get_quote(
            &plugin, &route, &token_in, &token_out,
            &0, &0, &0, &6,
        );
        (env, client)
    }

    #[test]
    fn test_estimate_fee_normal_load() {
        let (_, client) = setup();
        let req = FeeEstimateRequest {
            amount: 1_000_000,
            fee_bps: 30,
            network_load_bps: 5_000,
        };
        let resp = client.estimate_fee(&req);
        assert!(!resp.surge_pricing);
        assert_eq!(resp.protocol_fee, 3_000); // 1_000_000 * 30 / 10_000
        assert_eq!(resp.network_fee, 100);
        assert_eq!(resp.total_fee, 3_100);
        assert_eq!(resp.effective_fee_bps, 30);
    }

    #[test]
    fn test_estimate_fee_high_load_surge() {
        let (_, client) = setup();
        let req = FeeEstimateRequest {
            amount: 1_000_000,
            fee_bps: 30,
            network_load_bps: 9_000,
        };
        let resp = client.estimate_fee(&req);
        assert!(resp.surge_pricing);
        assert_eq!(resp.network_fee, 200); // 2× base
        assert_eq!(resp.effective_fee_bps, 60); // 2× fee_bps
    }

    #[test]
    fn test_estimate_fee_invalid_amount() {
        let (_, client) = setup();
        let req = FeeEstimateRequest { amount: 0, fee_bps: 30, network_load_bps: 0 };
        let result = client.try_estimate_fee(&req);
        assert_eq!(result, Err(Ok(QuoteError::InvalidAmount)));
    }

    #[test]
    fn test_invalid_precision_fails() {
        let (env, client, plugin) = setup();
        let token_in = Address::generate(&env);
        let token_out = Address::generate(&env);
        let route = String::from_str(&env, "mock/pool");
        let result = client.try_get_quote(
            &plugin, &route, &token_in, &token_out,
            &1_000_000, &0, &0, &0,
        );
        assert_eq!(result, Err(Ok(QuoteError::InvalidPrecision)));
    }

    #[test]
    fn test_invalid_slippage_fails() {
        let (env, client, plugin) = setup();
        let token_in = Address::generate(&env);
        let token_out = Address::generate(&env);
        let route = String::from_str(&env, "mock/pool");
        let result = client.try_get_quote(
            &plugin, &route, &token_in, &token_out,
            &1_000_000, &0, &10_001, &6,
        );
        assert_eq!(result, Err(Ok(QuoteError::InvalidSlippage)));
    }

    #[test]
    fn test_estimate_fee_normal_load() {
        let (_, client, _) = setup();
        let resp = client.estimate_fee(&1_000_000, &30, &5_000);
        assert!(!resp.surge_pricing);
        assert_eq!(resp.protocol_fee, 3_000);
        assert_eq!(resp.network_fee, 100);
        assert_eq!(resp.total_fee, 3_100);
        assert_eq!(resp.effective_fee_bps, 30);
    }

    #[test]
    fn test_estimate_fee_surge_pricing() {
        let (_, client, _, _) = setup();
        let (_, client, _) = setup();
        let resp = client.estimate_fee(&1_000_000, &30, &9_000);
        assert!(resp.surge_pricing);
        assert_eq!(resp.network_fee, 200);
        assert_eq!(resp.effective_fee_bps, 60);
    }

    #[test]
    fn test_estimate_fee_invalid_amount() {
        let (_, client, _, _) = setup();
        let r = client.try_estimate_fee(&0, &30, &0);
        assert_eq!(r, Err(Ok(QuoteError::InvalidAmount)));
}
    fn test_estimate_fees_skips_invalid() {
        let (env, client) = setup();
        let requests = soroban_sdk::vec![
            &env,
            FeeEstimateRequest { amount: 1_000_000, fee_bps: 30, network_load_bps: 0 },
            FeeEstimateRequest { amount: 0, fee_bps: 30, network_load_bps: 0 }, // invalid
            FeeEstimateRequest { amount: 500_000, fee_bps: 10, network_load_bps: 0 },
        ];
        let responses = client.estimate_fees(&requests);
        // Only 2 valid requests
        assert_eq!(responses.len(), 2);
    }
}

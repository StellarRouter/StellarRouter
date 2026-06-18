#![no_std]

//! # router-quote
//!
//! Quote calculation, route comparison, and price-impact filtering for the
//! stellar-router suite.
//!
//! ## Features
//! - Configurable fee basis points (fee_bps) per route
//! - Price impact calculation (bps) per quote
//! - `compare_quotes`: filter by max price-impact threshold + sort by best output
//! - Multiple quote comparison and best-route selection
//! - Admin-gated fee management with ownership transfer support
//!
//! ## Price Impact
//! Price impact is expressed in basis points (bps):
//!
//! ```text
//! price_impact_bps = fee_amount × 10_000 / amount_in
//! ```
//!
//! For a 1% fee route: `price_impact_bps = 100`.
//! `compare_quotes` rejects quotes whose `price_impact_bps > max_price_impact_bps`,
//! then returns the survivors sorted by `amount_out` descending (best route first).

use soroban_sdk::{
    contract, contracterror, contractimpl, contracttype, Address, Env, String, Symbol, Vec,
};

// ── Storage Keys ──────────────────────────────────────────────────────────────

#[contracttype]
pub enum DataKey {
    Admin,
    /// Route name -> fee in basis points (1 bps = 0.01%)
    RouteFee(String),
    /// Default fee if route-specific fee not set (in basis points)
    DefaultFee,
}

// ── Types ─────────────────────────────────────────────────────────────────────

#[contracttype]
#[derive(Clone, Debug, PartialEq)]
pub struct QuoteRequest {
    /// Route name to get quote for
    pub route: String,
    /// Input token address
    pub token_in: Address,
    /// Output token address
    pub token_out: Address,
    /// Amount of input token
    pub amount_in: i128,
}

#[contracttype]
#[derive(Clone, Debug, PartialEq)]
pub struct QuoteResponse {
    /// Route name
    pub route: String,
    /// Input token address
    pub token_in: Address,
    /// Output token address
    pub token_out: Address,
    /// Amount of input token
    pub amount_in: i128,
    /// Expected output amount after fees
    pub amount_out: i128,
    /// Fee amount deducted (in input token units)
    pub fee_amount: i128,
    /// Fee in basis points used for this quote
    pub fee_bps: u32,
    /// Price impact in basis points: `fee_amount × 10_000 / amount_in`.
    ///
    /// For simple fee-based routes this equals `fee_bps` numerically, but it is
    /// exposed as a first-class field so callers can apply threshold filters
    /// (e.g. via `compare_quotes`) without recomputing or knowing the fee model.
    pub price_impact_bps: i128,
}

// ── Errors ────────────────────────────────────────────────────────────────────

#[contracterror]
#[derive(Copy, Clone, Debug, PartialEq)]
pub enum QuoteError {
    AlreadyInitialized = 1,
    NotInitialized = 2,
    Unauthorized = 3,
    InvalidAmount = 4,
    InvalidFeeBps = 5,
    NoQuotesProvided = 6,
    RouteNotFound = 7,
    /// `max_price_impact_bps` argument is outside `[0, 10_000]`.
    InvalidPriceImpactBps = 8,
}

// ── Contract ──────────────────────────────────────────────────────────────────

#[contract]
pub struct RouterQuote;

#[contractimpl]
impl RouterQuote {
    /// Initialize the quote contract with an admin address and default fee.
    ///
    /// # Arguments
    /// * `env` — The Soroban environment.
    /// * `admin` — Address that will hold admin privileges.
    /// * `default_fee_bps` — Default fee in basis points (max 10 000 = 100 %).
    ///
    /// # Errors
    /// * [`QuoteError::AlreadyInitialized`] — contract already initialized.
    /// * [`QuoteError::InvalidFeeBps`] — `default_fee_bps > 10_000`.
    pub fn initialize(
        env: Env,
        admin: Address,
        default_fee_bps: u32,
    ) -> Result<(), QuoteError> {
        if env.storage().instance().has(&DataKey::Admin) {
            return Err(QuoteError::AlreadyInitialized);
        }
        if default_fee_bps > 10000 {
            return Err(QuoteError::InvalidFeeBps);
        }
        env.storage().instance().set(&DataKey::Admin, &admin);
        env.storage()
            .instance()
            .set(&DataKey::DefaultFee, &default_fee_bps);
        env.events().publish(
            (Symbol::new(&env, "initialized"),),
            (admin, default_fee_bps),
        );
        Ok(())
    }

    /// Set fee in basis points for a specific route.
    ///
    /// # Errors
    /// * [`QuoteError::NotInitialized`] — contract not yet initialized.
    /// * [`QuoteError::Unauthorized`] — caller is not the admin.
    /// * [`QuoteError::InvalidFeeBps`] — `fee_bps > 10_000`.
    pub fn set_route_fee(
        env: Env,
        caller: Address,
        route: String,
        fee_bps: u32,
    ) -> Result<(), QuoteError> {
        caller.require_auth();
        Self::require_admin(&env, &caller)?;
        if fee_bps > 10000 {
            return Err(QuoteError::InvalidFeeBps);
        }
        env.storage()
            .instance()
            .set(&DataKey::RouteFee(route.clone()), &fee_bps);
        env.events().publish(
            (Symbol::new(&env, "route_fee_set"),),
            (route, fee_bps),
        );
        Ok(())
    }

    /// Get fee in basis points for a specific route.
    ///
    /// Returns the route-specific fee if set, otherwise the default fee.
    pub fn get_route_fee(env: Env, route: String) -> u32 {
        env.storage()
            .instance()
            .get::<DataKey, u32>(&DataKey::RouteFee(route))
            .unwrap_or_else(|| {
                env.storage()
                    .instance()
                    .get(&DataKey::DefaultFee)
                    .unwrap_or(100)
            })
    }

    /// Get a quote for a single route with configurable fee.
    ///
    /// Computes `fee_amount`, `amount_out`, and `price_impact_bps` for the
    /// given `amount_in` and the route's configured fee.
    ///
    /// # Errors
    /// * [`QuoteError::InvalidAmount`] — if `amount_in <= 0`.
    pub fn get_quote(env: Env, request: QuoteRequest) -> Result<QuoteResponse, QuoteError> {
        if request.amount_in <= 0 {
            return Err(QuoteError::InvalidAmount);
        }
        let fee_bps = Self::get_route_fee(env.clone(), request.route.clone());
        let fee_amount = request
            .amount_in
            .checked_mul(fee_bps as i128)
            .and_then(|v| v.checked_div(10000))
            .unwrap_or(0);
        let amount_out = request.amount_in.checked_sub(fee_amount).unwrap_or(0);
        let price_impact_bps = fee_amount
            .checked_mul(10000)
            .and_then(|v| v.checked_div(request.amount_in))
            .unwrap_or(0);
        let response = QuoteResponse {
            route: request.route.clone(),
            token_in: request.token_in,
            token_out: request.token_out,
            amount_in: request.amount_in,
            amount_out,
            fee_amount,
            fee_bps,
            price_impact_bps,
        };
        env.events().publish(
            (Symbol::new(&env, "quote_calculated"),),
            (request.route, amount_out, fee_amount, price_impact_bps),
        );
        Ok(response)
    }

    /// Get quotes for multiple routes.
    ///
    /// # Errors
    /// * [`QuoteError::NoQuotesProvided`] — if `requests` is empty.
    /// * [`QuoteError::InvalidAmount`] — if any `amount_in <= 0`.
    pub fn get_quotes(
        env: Env,
        requests: Vec<QuoteRequest>,
    ) -> Result<Vec<QuoteResponse>, QuoteError> {
        if requests.is_empty() {
            return Err(QuoteError::NoQuotesProvided);
        }
        let mut responses = Vec::new(&env);
        for request in requests.iter() {
            let response = Self::get_quote(env.clone(), request)?;
            responses.push_back(response);
        }
        Ok(responses)
    }

    /// Get the best quote from multiple routes (highest `amount_out`).
    ///
    /// # Errors
    /// * [`QuoteError::NoQuotesProvided`] — if `requests` is empty.
    /// * [`QuoteError::InvalidAmount`] — if any `amount_in <= 0`.
    pub fn get_best_quote(
        env: Env,
        requests: Vec<QuoteRequest>,
    ) -> Result<QuoteResponse, QuoteError> {
        let quotes = Self::get_quotes(env.clone(), requests)?;
        let mut best_quote = quotes.get(0).unwrap();
        for i in 1..quotes.len() {
            let quote = quotes.get(i).unwrap();
            if quote.amount_out > best_quote.amount_out {
                best_quote = quote;
            }
        }
        env.events().publish(
            (Symbol::new(&env, "best_quote_selected"),),
            (best_quote.route.clone(), best_quote.amount_out),
        );
        Ok(best_quote)
    }

    /// Compare quotes across routes, filtered by price-impact threshold.
    ///
    /// Evaluates all `requests`, discards any route whose computed
    /// `price_impact_bps > max_price_impact_bps`, and returns the survivors
    /// sorted by `amount_out` descending (best route first).
    ///
    /// Returns an empty vector when every route exceeds the threshold — the
    /// caller should treat this as "no acceptable route found" and handle
    /// accordingly rather than proceeding with a high-slippage execution.
    ///
    /// ## Price Impact Formula
    /// ```text
    /// price_impact_bps = fee_amount × 10_000 / amount_in
    /// ```
    /// 100 bps = 1 % impact.  A threshold of `50` accepts only routes with
    /// ≤ 0.5 % price impact.
    ///
    /// # Arguments
    /// * `env` — The Soroban environment.
    /// * `requests` — Routes to evaluate; must be non-empty.
    /// * `max_price_impact_bps` — Inclusive upper bound on price impact.
    ///   Must be in `[0, 10_000]`.
    ///
    /// # Returns
    /// Filtered `Vec<QuoteResponse>` sorted by `amount_out` descending.
    /// May be empty if all routes exceed the threshold.
    ///
    /// # Errors
    /// * [`QuoteError::NoQuotesProvided`] — `requests` is empty.
    /// * [`QuoteError::InvalidAmount`] — any `amount_in <= 0`.
    /// * [`QuoteError::InvalidPriceImpactBps`] — `max_price_impact_bps` not in `[0, 10_000]`.
    pub fn compare_quotes(
        env: Env,
        requests: Vec<QuoteRequest>,
        max_price_impact_bps: i128,
    ) -> Result<Vec<QuoteResponse>, QuoteError> {
        if max_price_impact_bps < 0 || max_price_impact_bps > 10000 {
            return Err(QuoteError::InvalidPriceImpactBps);
        }
        let quotes = Self::get_quotes(env.clone(), requests)?;
        let mut passing: Vec<QuoteResponse> = Vec::new(&env);
        for i in 0..quotes.len() {
            let quote = quotes.get(i).unwrap();
            if quote.price_impact_bps <= max_price_impact_bps {
                passing.push_back(quote);
            }
        }
        let sorted = Self::sort_by_amount_out_desc(&env, passing);
        env.events().publish(
            (Symbol::new(&env, "quotes_compared"),),
            (sorted.len(), max_price_impact_bps),
        );
        Ok(sorted)
    }

    /// Update the default fee in basis points.
    ///
    /// # Errors
    /// * [`QuoteError::NotInitialized`] — contract not yet initialized.
    /// * [`QuoteError::Unauthorized`] — caller is not the admin.
    /// * [`QuoteError::InvalidFeeBps`] — `fee_bps > 10_000`.
    pub fn set_default_fee(env: Env, caller: Address, fee_bps: u32) -> Result<(), QuoteError> {
        caller.require_auth();
        Self::require_admin(&env, &caller)?;
        if fee_bps > 10000 {
            return Err(QuoteError::InvalidFeeBps);
        }
        env.storage().instance().set(&DataKey::DefaultFee, &fee_bps);
        env.events()
            .publish((Symbol::new(&env, "default_fee_updated"),), fee_bps);
        Ok(())
    }

    /// Get the current default fee in basis points.
    pub fn get_default_fee(env: Env) -> u32 {
        env.storage()
            .instance()
            .get(&DataKey::DefaultFee)
            .unwrap_or(100)
    }

    /// Get current admin address.
    pub fn admin(env: Env) -> Address {
        env.storage()
            .instance()
            .get(&DataKey::Admin)
            .expect("not initialized")
    }

    /// Transfer admin privileges to a new address.
    ///
    /// # Errors
    /// * [`QuoteError::NotInitialized`] — contract not yet initialized.
    /// * [`QuoteError::Unauthorized`] — `current` is not the admin.
    pub fn transfer_admin(
        env: Env,
        current: Address,
        new_admin: Address,
    ) -> Result<(), QuoteError> {
        current.require_auth();
        Self::require_admin(&env, &current)?;
        env.storage().instance().set(&DataKey::Admin, &new_admin);
        env.events().publish(
            (Symbol::new(&env, "admin_transferred"),),
            (current, new_admin),
        );
        Ok(())
    }

    // ── Helpers ───────────────────────────────────────────────────────────────

    /// Verify `caller` is the stored admin, returning `NotInitialized` when
    /// the contract has not yet been initialized and `Unauthorized` when caller
    /// does not match.  Does NOT panic.
    fn require_admin(env: &Env, caller: &Address) -> Result<(), QuoteError> {
        let admin: Address = env
            .storage()
            .instance()
            .get(&DataKey::Admin)
            .ok_or(QuoteError::NotInitialized)?;
        if &admin != caller {
            return Err(QuoteError::Unauthorized);
        }
        Ok(())
    }

    /// Return a new `Vec<QuoteResponse>` with elements ordered by `amount_out`
    /// descending (best output first).
    ///
    /// Uses insertion sort — O(n²) — which is correct and efficient for the
    /// small route sets typical in DeFi routing (2–20 elements).
    /// `soroban_sdk::Vec` has no built-in sort, so each insertion rebuilds the
    /// vector to keep contract storage access patterns deterministic.
    fn sort_by_amount_out_desc(env: &Env, quotes: Vec<QuoteResponse>) -> Vec<QuoteResponse> {
        let n = quotes.len();
        let mut sorted: Vec<QuoteResponse> = Vec::new(env);
        for i in 0..n {
            let item = quotes.get(i).unwrap();
            let mut new_sorted: Vec<QuoteResponse> = Vec::new(env);
            let mut inserted = false;
            for j in 0..sorted.len() {
                let existing = sorted.get(j).unwrap();
                if !inserted && item.amount_out >= existing.amount_out {
                    new_sorted.push_back(item.clone());
                    inserted = true;
                }
                new_sorted.push_back(existing);
            }
            if !inserted {
                new_sorted.push_back(item);
            }
            sorted = new_sorted;
        }
        sorted
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use soroban_sdk::{testutils::Address as _, Env, String};

    fn setup() -> (Env, Address, RouterQuoteClient<'static>) {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register_contract(None, RouterQuote);
        let client = RouterQuoteClient::new(&env, &contract_id);
        let admin = Address::generate(&env);
        client.initialize(&admin, &100); // 1% default fee
        (env, admin, client)
    }

    // ── Initialization ────────────────────────────────────────────────────────

    #[test]
    fn test_initialize() {
        let (_env, admin, client) = setup();
        assert_eq!(client.admin(), admin);
        assert_eq!(client.get_default_fee(), 100);
    }

    #[test]
    fn test_initialize_twice_fails() {
        let (_env, admin, client) = setup();
        let result = client.try_initialize(&admin, &100);
        assert_eq!(result, Err(Ok(QuoteError::AlreadyInitialized)));
    }

    #[test]
    fn test_initialize_invalid_fee_fails() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register_contract(None, RouterQuote);
        let client = RouterQuoteClient::new(&env, &contract_id);
        let admin = Address::generate(&env);
        let result = client.try_initialize(&admin, &10001);
        assert_eq!(result, Err(Ok(QuoteError::InvalidFeeBps)));
    }

    // ── Route Fee Management ──────────────────────────────────────────────────

    #[test]
    fn test_set_and_get_route_fee() {
        let (env, admin, client) = setup();
        let route = String::from_str(&env, "uniswap");
        client.set_route_fee(&admin, &route, &50); // 0.5%
        assert_eq!(client.get_route_fee(&route), 50);
    }

    #[test]
    fn test_get_route_fee_returns_default_when_not_set() {
        let (env, _admin, client) = setup();
        let route = String::from_str(&env, "uniswap");
        assert_eq!(client.get_route_fee(&route), 100); // Default 1%
    }

    #[test]
    fn test_unauthorized_set_route_fee_fails() {
        let (env, _admin, client) = setup();
        let unauthorized = Address::generate(&env);
        let route = String::from_str(&env, "uniswap");
        let result = client.try_set_route_fee(&unauthorized, &route, &50);
        assert_eq!(result, Err(Ok(QuoteError::Unauthorized)));
    }

    // ── get_quote ─────────────────────────────────────────────────────────────

    #[test]
    fn test_get_quote_with_default_fee() {
        let (env, _admin, client) = setup();
        let token_in = Address::generate(&env);
        let token_out = Address::generate(&env);
        let request = QuoteRequest {
            route: String::from_str(&env, "uniswap"),
            token_in: token_in.clone(),
            token_out: token_out.clone(),
            amount_in: 10000,
        };
        let response = client.get_quote(&request);
        assert_eq!(response.amount_in, 10000);
        assert_eq!(response.fee_bps, 100);
        assert_eq!(response.fee_amount, 100); // 10000 * 100 / 10000
        assert_eq!(response.amount_out, 9900);
        assert_eq!(response.price_impact_bps, 100); // 100 * 10000 / 10000
    }

    #[test]
    fn test_get_quote_with_custom_route_fee() {
        let (env, admin, client) = setup();
        let route = String::from_str(&env, "sushiswap");
        client.set_route_fee(&admin, &route, &30); // 0.3%
        let token_in = Address::generate(&env);
        let token_out = Address::generate(&env);
        let request = QuoteRequest {
            route: route.clone(),
            token_in,
            token_out,
            amount_in: 10000,
        };
        let response = client.get_quote(&request);
        assert_eq!(response.fee_bps, 30);
        assert_eq!(response.fee_amount, 30);
        assert_eq!(response.amount_out, 9970);
        assert_eq!(response.price_impact_bps, 30);
    }

    #[test]
    fn test_get_quote_price_impact_bps_matches_fee_bps() {
        // For simple fee-based routes, price_impact_bps == fee_bps.
        let (env, admin, client) = setup();
        let route = String::from_str(&env, "curve");
        client.set_route_fee(&admin, &route, &75); // 0.75%
        let request = QuoteRequest {
            route,
            token_in: Address::generate(&env),
            token_out: Address::generate(&env),
            amount_in: 1_000_000,
        };
        let response = client.get_quote(&request);
        assert_eq!(response.price_impact_bps, response.fee_bps as i128);
    }

    #[test]
    fn test_get_quote_invalid_amount_fails() {
        let (env, _admin, client) = setup();
        let request = QuoteRequest {
            route: String::from_str(&env, "uniswap"),
            token_in: Address::generate(&env),
            token_out: Address::generate(&env),
            amount_in: 0,
        };
        let result = client.try_get_quote(&request);
        assert_eq!(result, Err(Ok(QuoteError::InvalidAmount)));
    }

    // ── get_quotes ────────────────────────────────────────────────────────────

    #[test]
    fn test_get_quotes_multiple_routes() {
        let (env, admin, client) = setup();
        let route1 = String::from_str(&env, "uniswap");
        let route2 = String::from_str(&env, "sushiswap");
        client.set_route_fee(&admin, &route1, &100); // 1%
        client.set_route_fee(&admin, &route2, &30); // 0.3%
        let token_in = Address::generate(&env);
        let token_out = Address::generate(&env);
        let mut requests = Vec::new(&env);
        requests.push_back(QuoteRequest {
            route: route1.clone(),
            token_in: token_in.clone(),
            token_out: token_out.clone(),
            amount_in: 10000,
        });
        requests.push_back(QuoteRequest {
            route: route2.clone(),
            token_in: token_in.clone(),
            token_out: token_out.clone(),
            amount_in: 10000,
        });
        let responses = client.get_quotes(&requests);
        assert_eq!(responses.len(), 2);
        let resp1 = responses.get(0).unwrap();
        assert_eq!(resp1.route, route1);
        assert_eq!(resp1.amount_out, 9900);
        assert_eq!(resp1.price_impact_bps, 100);
        let resp2 = responses.get(1).unwrap();
        assert_eq!(resp2.route, route2);
        assert_eq!(resp2.amount_out, 9970);
        assert_eq!(resp2.price_impact_bps, 30);
    }

    #[test]
    fn test_get_quotes_empty_fails() {
        let (env, _admin, client) = setup();
        let requests = Vec::new(&env);
        let result = client.try_get_quotes(&requests);
        assert_eq!(result, Err(Ok(QuoteError::NoQuotesProvided)));
    }

    // ── get_best_quote ────────────────────────────────────────────────────────

    #[test]
    fn test_get_best_quote() {
        let (env, admin, client) = setup();
        let route1 = String::from_str(&env, "uniswap");
        let route2 = String::from_str(&env, "sushiswap");
        let route3 = String::from_str(&env, "pancakeswap");
        client.set_route_fee(&admin, &route1, &100); // 1%
        client.set_route_fee(&admin, &route2, &30); // 0.3% — best
        client.set_route_fee(&admin, &route3, &50); // 0.5%
        let token_in = Address::generate(&env);
        let token_out = Address::generate(&env);
        let mut requests = Vec::new(&env);
        requests.push_back(QuoteRequest {
            route: route1,
            token_in: token_in.clone(),
            token_out: token_out.clone(),
            amount_in: 10000,
        });
        requests.push_back(QuoteRequest {
            route: route2.clone(),
            token_in: token_in.clone(),
            token_out: token_out.clone(),
            amount_in: 10000,
        });
        requests.push_back(QuoteRequest {
            route: route3,
            token_in: token_in.clone(),
            token_out: token_out.clone(),
            amount_in: 10000,
        });
        let best = client.get_best_quote(&requests);
        assert_eq!(best.route, route2);
        assert_eq!(best.amount_out, 9970);
        assert_eq!(best.fee_bps, 30);
    }

    #[test]
    fn test_get_best_quote_empty_fails() {
        let (env, _admin, client) = setup();
        let requests = Vec::new(&env);
        let result = client.try_get_best_quote(&requests);
        assert_eq!(result, Err(Ok(QuoteError::NoQuotesProvided)));
    }

    // ── compare_quotes ────────────────────────────────────────────────────────

    fn make_request(env: &Env, route: &str, fee_bps: u32, amount_in: i128, admin: &Address, client: &RouterQuoteClient) -> QuoteRequest {
        let r = String::from_str(env, route);
        client.set_route_fee(admin, &r, &fee_bps);
        QuoteRequest {
            route: r,
            token_in: Address::generate(env),
            token_out: Address::generate(env),
            amount_in,
        }
    }

    #[test]
    fn test_compare_quotes_filters_by_price_impact() {
        let (env, admin, client) = setup();
        // route A: 1% fee → price_impact_bps = 100
        // route B: 0.3% fee → price_impact_bps = 30
        let mut requests = Vec::new(&env);
        requests.push_back(make_request(&env, "route_a", 100, 10000, &admin, &client));
        requests.push_back(make_request(&env, "route_b", 30, 10000, &admin, &client));
        // threshold 50 → only route_b passes
        let result = client.compare_quotes(&requests, &50_i128);
        assert_eq!(result.len(), 1);
        assert_eq!(result.get(0).unwrap().fee_bps, 30);
    }

    #[test]
    fn test_compare_quotes_returns_empty_when_all_exceed_threshold() {
        let (env, admin, client) = setup();
        let mut requests = Vec::new(&env);
        requests.push_back(make_request(&env, "route_a", 200, 10000, &admin, &client));
        requests.push_back(make_request(&env, "route_b", 300, 10000, &admin, &client));
        // threshold 50 → none pass
        let result = client.compare_quotes(&requests, &50_i128);
        assert_eq!(result.len(), 0);
    }

    #[test]
    fn test_compare_quotes_sorts_by_amount_out_desc() {
        let (env, admin, client) = setup();
        // Three routes with different fees → different amount_outs.
        // 0.3% fee: amount_out = 9970 (best)
        // 0.5% fee: amount_out = 9950
        // 1.0% fee: amount_out = 9900 (worst)
        let mut requests = Vec::new(&env);
        requests.push_back(make_request(&env, "route_worst",  100, 10000, &admin, &client));
        requests.push_back(make_request(&env, "route_best",    30, 10000, &admin, &client));
        requests.push_back(make_request(&env, "route_middle",  50, 10000, &admin, &client));
        // All pass threshold 10000
        let result = client.compare_quotes(&requests, &10000_i128);
        assert_eq!(result.len(), 3);
        assert_eq!(result.get(0).unwrap().amount_out, 9970); // best first
        assert_eq!(result.get(1).unwrap().amount_out, 9950);
        assert_eq!(result.get(2).unwrap().amount_out, 9900); // worst last
    }

    #[test]
    fn test_compare_quotes_accepts_exact_threshold_match() {
        let (env, admin, client) = setup();
        // 1% fee → price_impact_bps = 100; threshold = 100 (boundary inclusive)
        let mut requests = Vec::new(&env);
        requests.push_back(make_request(&env, "route_a", 100, 10000, &admin, &client));
        let result = client.compare_quotes(&requests, &100_i128);
        assert_eq!(result.len(), 1);
    }

    #[test]
    fn test_compare_quotes_all_pass_preserves_sort() {
        let (env, admin, client) = setup();
        let mut requests = Vec::new(&env);
        requests.push_back(make_request(&env, "route_a", 50, 10000, &admin, &client)); // 9950 out
        requests.push_back(make_request(&env, "route_b", 20, 10000, &admin, &client)); // 9980 out
        // Both pass threshold 10000
        let result = client.compare_quotes(&requests, &10000_i128);
        assert_eq!(result.len(), 2);
        // route_b (lower fee) should be first
        assert_eq!(result.get(0).unwrap().amount_out, 9980);
        assert_eq!(result.get(1).unwrap().amount_out, 9950);
    }

    #[test]
    fn test_compare_quotes_invalid_threshold_too_high_fails() {
        let (env, admin, client) = setup();
        let mut requests = Vec::new(&env);
        requests.push_back(make_request(&env, "route_a", 100, 10000, &admin, &client));
        let result = client.try_compare_quotes(&requests, &10001_i128);
        assert_eq!(result, Err(Ok(QuoteError::InvalidPriceImpactBps)));
    }

    #[test]
    fn test_compare_quotes_invalid_threshold_negative_fails() {
        let (env, admin, client) = setup();
        let mut requests = Vec::new(&env);
        requests.push_back(make_request(&env, "route_a", 100, 10000, &admin, &client));
        let result = client.try_compare_quotes(&requests, &(-1_i128));
        assert_eq!(result, Err(Ok(QuoteError::InvalidPriceImpactBps)));
    }

    #[test]
    fn test_compare_quotes_empty_requests_fails() {
        let (env, _admin, client) = setup();
        let requests: Vec<QuoteRequest> = Vec::new(&env);
        let result = client.try_compare_quotes(&requests, &100_i128);
        assert_eq!(result, Err(Ok(QuoteError::NoQuotesProvided)));
    }

    #[test]
    fn test_compare_quotes_zero_threshold_filters_all_nonzero_fee() {
        // Only routes with 0-bps fee would pass a threshold of 0.
        // A 1-bps fee produces price_impact_bps = 1 > 0, so it's filtered.
        let (env, admin, client) = setup();
        let mut requests = Vec::new(&env);
        requests.push_back(make_request(&env, "route_a", 1, 10000, &admin, &client));
        requests.push_back(make_request(&env, "route_b", 0, 10000, &admin, &client));
        let result = client.compare_quotes(&requests, &0_i128);
        // route_b (0-bps fee → price_impact_bps = 0) should be the only one
        assert_eq!(result.len(), 1);
        assert_eq!(result.get(0).unwrap().fee_bps, 0);
        assert_eq!(result.get(0).unwrap().price_impact_bps, 0);
    }

    // ── Admin / Governance ────────────────────────────────────────────────────

    #[test]
    fn test_set_default_fee() {
        let (_env, admin, client) = setup();
        client.set_default_fee(&admin, &200); // 2%
        assert_eq!(client.get_default_fee(), 200);
    }

    #[test]
    fn test_set_default_fee_invalid_fails() {
        let (_env, admin, client) = setup();
        let result = client.try_set_default_fee(&admin, &10001);
        assert_eq!(result, Err(Ok(QuoteError::InvalidFeeBps)));
    }

    #[test]
    fn test_transfer_admin() {
        let (env, admin, client) = setup();
        let new_admin = Address::generate(&env);
        client.transfer_admin(&admin, &new_admin);
        assert_eq!(client.admin(), new_admin);
    }

    #[test]
    fn test_admin_getter() {
        let (_env, admin, client) = setup();
        assert_eq!(client.admin(), admin);
    }

    #[test]
    fn test_require_admin_returns_not_initialized_before_init() {
        // Before initialize, require_admin (called by set_default_fee) must
        // return NotInitialized rather than panicking.
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register_contract(None, RouterQuote);
        let client = RouterQuoteClient::new(&env, &contract_id);
        let caller = Address::generate(&env);
        let result = client.try_set_default_fee(&caller, &100);
        assert_eq!(result, Err(Ok(QuoteError::NotInitialized)));
    }
}

#![no_std]

//! # router-quote
//!
//! Quote calculation and route comparison for the stellar-router suite.
//! Provides configurable fee-based quote calculations and best-route selection.
//!
//! ## Features
//! - Configurable fee basis points (fee_bps) per route
//! - Multiple quote comparison
//! - Best quote selection based on highest output amount
//! - Integration with liquidity plugins

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
}

// ── Contract ──────────────────────────────────────────────────────────────────

#[contract]
pub struct RouterQuote;

#[contractimpl]
impl RouterQuote {
    /// Initialize the quote contract with an admin address and default fee.
    ///
    /// Sets up the admin and default fee in basis points (bps).
    /// 1 bps = 0.01%, so 100 bps = 1%.
    ///
    /// # Arguments
    /// * `env` - The Soroban environment.
    /// * `admin` - The address that will have admin privileges.
    /// * `default_fee_bps` - Default fee in basis points (max 10000 = 100%).
    ///
    /// # Returns
    /// `Ok(())` on success.
    ///
    /// # Errors
    /// * [`QuoteError::AlreadyInitialized`] — if already initialized.
    /// * [`QuoteError::InvalidFeeBps`] — if fee_bps > 10000.
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

    /// Set fee in basis points for a specific route.
    ///
    /// Allows admin to configure per-route fees. If not set, the default fee
    /// is used.
    ///
    /// # Arguments
    /// * `env` - The Soroban environment.
    /// * `caller` - The address initiating the call; must be the admin.
    /// * `route` - The route name to configure.
    /// * `fee_bps` - Fee in basis points (max 10000 = 100%).
    ///
    /// # Returns
    /// `Ok(())` on success.
    ///
    /// # Errors
    /// * [`QuoteError::Unauthorized`] — if caller is not the admin.
    /// * [`QuoteError::InvalidFeeBps`] — if fee_bps > 10000.
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
    /// Returns the route-specific fee if set, otherwise returns the default fee.
    ///
    /// # Arguments
    /// * `env` - The Soroban environment.
    /// * `route` - The route name.
    ///
    /// # Returns
    /// Fee in basis points.
    pub fn get_route_fee(env: Env, route: String) -> u32 {
        env.storage()
            .instance()
            .get::<DataKey, u32>(&DataKey::RouteFee(route))
            .unwrap_or_else(|| {
                env.storage()
                    .instance()
                    .get(&DataKey::DefaultFee)
                    .unwrap_or(100) // Default to 1% if not initialized
            })
    }

    /// Get a quote for a single route with configurable fee.
    ///
    /// Calculates the expected output amount after deducting fees based on
    /// the route's configured fee_bps.
    ///
    /// # Arguments
    /// * `env` - The Soroban environment.
    /// * `request` - Quote request containing route, tokens, and amount.
    ///
    /// # Returns
    /// [`QuoteResponse`] with calculated amounts and fees.
    ///
    /// # Errors
    /// * [`QuoteError::InvalidAmount`] — if amount_in <= 0.
    pub fn get_quote(env: Env, request: QuoteRequest) -> Result<QuoteResponse, QuoteError> {
        if request.amount_in <= 0 {
            return Err(QuoteError::InvalidAmount);
        }

        let fee_bps = Self::get_route_fee(env.clone(), request.route.clone());

        // Calculate fee: fee_amount = amount_in * fee_bps / 10000
        let fee_amount = request
            .amount_in
            .checked_mul(fee_bps as i128)
            .and_then(|v| v.checked_div(10000))
            .unwrap_or(0);

        // Calculate output: amount_out = amount_in - fee_amount
        let amount_out = request.amount_in.checked_sub(fee_amount).unwrap_or(0);

        let response = QuoteResponse {
            route: request.route.clone(),
            token_in: request.token_in,
            token_out: request.token_out,
            amount_in: request.amount_in,
            amount_out,
            fee_amount,
            fee_bps,
        };

        env.events().publish(
            (Symbol::new(&env, "quote_calculated"),),
            (request.route, amount_out, fee_amount),
        );

        Ok(response)
    }

    /// Get quotes for multiple routes.
    ///
    /// Calculates quotes for all provided requests and returns them as a vector.
    ///
    /// # Arguments
    /// * `env` - The Soroban environment.
    /// * `requests` - Vector of quote requests.
    ///
    /// # Returns
    /// Vector of [`QuoteResponse`] for each request.
    ///
    /// # Errors
    /// * [`QuoteError::NoQuotesProvided`] — if requests vector is empty.
    /// * [`QuoteError::InvalidAmount`] — if any amount_in <= 0.
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

    /// Get the best quote from multiple routes.
    ///
    /// Calculates quotes for all provided requests and returns the single
    /// quote with the highest amount_out. Useful for automatic route selection.
    ///
    /// # Arguments
    /// * `env` - The Soroban environment.
    /// * `requests` - Vector of quote requests to compare.
    ///
    /// # Returns
    /// The [`QuoteResponse`] with the highest amount_out.
    ///
    /// # Errors
    /// * [`QuoteError::NoQuotesProvided`] — if requests vector is empty.
    /// * [`QuoteError::InvalidAmount`] — if any amount_in <= 0.
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

    /// Update the default fee in basis points.
    ///
    /// Changes the default fee used when a route-specific fee is not set.
    ///
    /// # Arguments
    /// * `env` - The Soroban environment.
    /// * `caller` - The address initiating the call; must be the admin.
    /// * `fee_bps` - New default fee in basis points (max 10000 = 100%).
    ///
    /// # Returns
    /// `Ok(())` on success.
    ///
    /// # Errors
    /// * [`QuoteError::Unauthorized`] — if caller is not the admin.
    /// * [`QuoteError::InvalidFeeBps`] — if fee_bps > 10000.
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
    ///
    /// # Arguments
    /// * `env` - The Soroban environment.
    ///
    /// # Returns
    /// Default fee in basis points.
    pub fn get_default_fee(env: Env) -> u32 {
        env.storage()
            .instance()
            .get(&DataKey::DefaultFee)
            .unwrap_or(100) // Default to 1% if not initialized
    }

    /// Get current admin address.
    ///
    /// # Arguments
    /// * `env` - The Soroban environment.
    ///
    /// # Returns
    /// The admin [`Address`].
    pub fn admin(env: Env) -> Address {
        env.storage()
            .instance()
            .get(&DataKey::Admin)
            .expect("not initialized")
    }

    /// Transfer admin to a new address.
    ///
    /// # Arguments
    /// * `env` - The Soroban environment.
    /// * `current` - The current admin address; must authenticate.
    /// * `new_admin` - The address that will become the new admin.
    ///
    /// # Returns
    /// `Ok(())` on success.
    ///
    /// # Errors
    /// * [`QuoteError::Unauthorized`] — if current is not the admin.
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

    fn require_admin(env: &Env, caller: &Address) -> Result<(), QuoteError> {
        let admin = Self::admin(env.clone());
        if &admin != caller {
            return Err(QuoteError::Unauthorized);
        }
        Ok(())
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use soroban_sdk::{
        testutils::{Address as _, Events},
        vec, Env, String,
    };

    fn setup() -> (Env, Address, RouterQuoteClient<'static>) {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register_contract(None, RouterQuote);
        let client = RouterQuoteClient::new(&env, &contract_id);
        let admin = Address::generate(&env);
        client.initialize(&admin, &100); // 1% default fee
        (env, admin, client)
    }

    #[test]
    fn test_initialize() {
        let (env, admin, client) = setup();
        assert_eq!(client.admin(), admin);
        assert_eq!(client.get_default_fee(), 100);
    }

    #[test]
    fn test_initialize_twice_fails() {
        let (env, admin, client) = setup();
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
        assert_eq!(response.fee_bps, 100); // 1%
        assert_eq!(response.fee_amount, 100); // 10000 * 100 / 10000 = 100
        assert_eq!(response.amount_out, 9900); // 10000 - 100 = 9900
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
            token_in: token_in.clone(),
            token_out: token_out.clone(),
            amount_in: 10000,
        };

        let response = client.get_quote(&request);
        assert_eq!(response.fee_bps, 30); // 0.3%
        assert_eq!(response.fee_amount, 30); // 10000 * 30 / 10000 = 30
        assert_eq!(response.amount_out, 9970); // 10000 - 30 = 9970
    }

    #[test]
    fn test_get_quote_invalid_amount_fails() {
        let (env, _admin, client) = setup();
        let token_in = Address::generate(&env);
        let token_out = Address::generate(&env);
        let request = QuoteRequest {
            route: String::from_str(&env, "uniswap"),
            token_in,
            token_out,
            amount_in: 0,
        };

        let result = client.try_get_quote(&request);
        assert_eq!(result, Err(Ok(QuoteError::InvalidAmount)));
    }

    #[test]
    fn test_get_quotes_multiple_routes() {
        let (env, admin, client) = setup();

        // Set different fees for different routes
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
        assert_eq!(resp1.amount_out, 9900); // 1% fee

        let resp2 = responses.get(1).unwrap();
        assert_eq!(resp2.route, route2);
        assert_eq!(resp2.amount_out, 9970); // 0.3% fee
    }

    #[test]
    fn test_get_quotes_empty_fails() {
        let (env, _admin, client) = setup();
        let requests = Vec::new(&env);
        let result = client.try_get_quotes(&requests);
        assert_eq!(result, Err(Ok(QuoteError::NoQuotesProvided)));
    }

    #[test]
    fn test_get_best_quote() {
        let (env, admin, client) = setup();

        // Set different fees for different routes
        let route1 = String::from_str(&env, "uniswap");
        let route2 = String::from_str(&env, "sushiswap");
        let route3 = String::from_str(&env, "pancakeswap");
        client.set_route_fee(&admin, &route1, &100); // 1%
        client.set_route_fee(&admin, &route2, &30); // 0.3% - best
        client.set_route_fee(&admin, &route3, &50); // 0.5%

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
        requests.push_back(QuoteRequest {
            route: route3.clone(),
            token_in: token_in.clone(),
            token_out: token_out.clone(),
            amount_in: 10000,
        });

        let best = client.get_best_quote(&requests);
        assert_eq!(best.route, route2); // sushiswap has lowest fee
        assert_eq!(best.amount_out, 9970); // Best output
        assert_eq!(best.fee_bps, 30);
    }

    #[test]
    fn test_get_best_quote_empty_fails() {
        let (env, _admin, client) = setup();
        let requests = Vec::new(&env);
        let result = client.try_get_best_quote(&requests);
        assert_eq!(result, Err(Ok(QuoteError::NoQuotesProvided)));
    }

    #[test]
    fn test_set_default_fee() {
        let (env, admin, client) = setup();
        client.set_default_fee(&admin, &200); // 2%
        assert_eq!(client.get_default_fee(), 200);
    }

    #[test]
    fn test_set_default_fee_invalid_fails() {
        let (env, admin, client) = setup();
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
    fn test_unauthorized_set_route_fee_fails() {
        let (env, _admin, client) = setup();
        let unauthorized = Address::generate(&env);
        let route = String::from_str(&env, "uniswap");
        let result = client.try_set_route_fee(&unauthorized, &route, &50);
        assert_eq!(result, Err(Ok(QuoteError::Unauthorized)));
    }

    #[test]
    fn test_admin_getter() {
        let (env, admin, client) = setup();
        assert_eq!(client.admin(), admin);
    }
}

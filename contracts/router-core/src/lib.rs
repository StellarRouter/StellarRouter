#![no_std]

//! # router-core
//!
//! Central dispatcher for the stellar-router suite.
//! Routes calls to registered contracts by name, enforces access control,
//! and delegates to the registry for address resolution.
//!
//! ## Features
//! - Route calls to contracts by name (resolved via registry)
//! - Admin-controlled route registration and removal
//! - Pause/unpause individual routes or all routing
//! - Event emission on every route operation
//!
//! ## Events (following naming convention: past tense verbs in snake_case)
//! - `route_registered` — Route registered (route_name, address)
//! - `route_updated` — Route updated (route_name)
//! - `route_overwritten` — Route overwritten by same name (route_name)
//! - `route_removed` — Route removed (route_name)
//! - `route_paused` — Route paused/unpaused (route_name, paused)
//! - `route_resolve_paused` — Route resolution paused (route_name)
//! - `routed` — Route resolved (route_name, address)
//! - `router_paused` — Router globally paused/unpaused (paused)
//! - `metadata_updated` — Route metadata updated (route_name, metadata)
//! - `alias_added` — Route alias added (existing_name, alias_name)
//! - `alias_removed` — Route alias removed (alias_name)
//! - `route_scored` — Route score updated (route_name, score)
//! - `best_route_selected` — Best route selected (route_name)
//! - `admin_transferred` — Admin transferred (old_admin, new_admin)

use soroban_sdk::{
    contract, contracterror, contractimpl, contracttype, Address, Env, String, Symbol, Vec,
};
extern crate alloc;
use alloc::string::ToString;

// ── Storage Keys ──────────────────────────────────────────────────────────────

#[contracttype]
pub enum DataKey {
    Admin,
    Route(String),    // name -> RouteEntry
    RouteNames,
    RouteCount,       // u32: O(1) counter kept in sync with RouteNames
    Paused,
    TotalRouted,
    Alias(String),    // alias -> original_name
    Aliases,          // Vec<String> of all alias names
    Score(String),    // name -> RouteScore
    Metadata(String), // name -> RouteMetadata (stored separately; avoids nested contracttype)
}

// ── Types ─────────────────────────────────────────────────────────────────────

#[contracttype]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RouteMetadata {
    /// Human-readable description (max 256 chars)
    pub description: String,
    /// Tags for categorization (max 5 tags)
    pub tags: Vec<String>,
    /// Owner address (use the zero/contract address as sentinel for "no owner")
    pub owner: Address,
}

#[contracttype]
#[derive(Clone, Debug, PartialEq)]
pub struct RouteEntry {
    /// Resolved contract address for this route
    pub address: Address,
    /// Human-readable route name
    pub name: String,
    /// Whether this specific route is paused
    pub paused: bool,
    /// Who last updated this route
    pub updated_by: Address,
}

#[contracttype]
#[derive(Clone, Debug, PartialEq)]
pub struct RouteRegisterInput {
    pub name: String,
    pub address: Address,
    pub metadata: Option<RouteMetadata>,
}


/// Scoring attributes for a route used in path selection.
///
/// Higher scores indicate more preferred routes. The composite score is
/// computed as: `liquidity_score + reliability_score - fee_bps / 10`.
/// All fields are set by the admin and reflect off-chain measurements.
#[contracttype]
#[derive(Clone, Debug, PartialEq)]
pub struct RouteScore {
    /// Liquidity depth score (0–100). Higher = more liquid.
    pub liquidity_score: u32,
    /// Fee rate in basis points (e.g., 30 = 0.30%). Lower = cheaper.
    pub fee_bps: u32,
    /// Historical reliability score (0–100). Higher = more reliable.
    pub reliability_score: u32,
}

/// Resolution-specific errors returned by [`RouterCore::batch_resolve`].
///
/// Mirrors the subset of [`RouterError`] variants that `resolve` can produce,
/// represented as a `contracttype` so it can be embedded in a `Vec`.
#[contracttype]
#[derive(Clone, Debug, PartialEq)]
pub enum ResolveError {
    RouterPaused,
    RouteNotFound,
    RoutePaused,
}

/// Per-entry result returned by [`RouterCore::batch_resolve`].
#[contracttype]
#[derive(Clone, Debug, PartialEq)]
pub enum BatchResolveResult {
    Ok(Address),
    Err(ResolveError),
}

// ── Errors ────────────────────────────────────────────────────────────────────

#[contracterror]
#[derive(Copy, Clone, Debug, PartialEq)]
pub enum RouterError {
    AlreadyInitialized = 1,
    NotInitialized = 2,
    Unauthorized = 3,
    RouteNotFound = 4,
    RoutePaused = 5,
    RouterPaused = 6,
    RouteAlreadyExists = 7,
    InvalidRouteName = 8,
    InvalidMetadata = 9,
}

// ── Contract ──────────────────────────────────────────────────────────────────

#[contract]
pub struct RouterCore;

#[contractimpl]
impl RouterCore {
    /// Initialize the router with an admin address.
    ///
    /// Sets up the admin, marks the router as unpaused, and resets the total
    /// routed counter to zero. Must be called exactly once before any other
    /// function.
    ///
    /// # Arguments
    /// * `env` - The Soroban environment.
    /// * `admin` - The address that will have admin privileges over this router.
    ///
    /// # Returns
    /// `Ok(())` on success.
    ///
    /// # Errors
    /// * [`RouterError::AlreadyInitialized`] — if the contract has already been initialized.
    pub fn initialize(env: Env, admin: Address) -> Result<(), RouterError> {
        if env.storage().instance().has(&DataKey::Admin) {
            return Err(RouterError::AlreadyInitialized);
        }
        env.storage().instance().set(&DataKey::Admin, &admin);
        env.storage()
            .instance()
            .set(&DataKey::RouteNames, &Vec::<String>::new(&env));
        env.storage()
            .instance()
            .set(&DataKey::Aliases, &Vec::<String>::new(&env));
        env.storage().instance().set(&DataKey::Paused, &false);
        env.storage().instance().set(&DataKey::TotalRouted, &0u64);
        env.storage().instance().set(&DataKey::RouteCount, &0u32);
        Ok(())
    }

    /// Register a new route by name pointing to a contract address.
    ///
    /// Associates a human-readable `name` with a target contract `address`.
    /// The route starts in an unpaused state. Caller must be the admin.
    ///
    /// # Arguments
    /// * `env` - The Soroban environment.
    /// * `caller` - The address initiating the call; must be the admin.
    /// * `name` - A unique human-readable identifier for the route. Must not be empty or whitespace-only.
    /// * `address` - The contract address this route resolves to.
    ///
    /// # Returns
    /// `Ok(())` on success.
    ///
    /// # Errors
    /// * [`RouterError::Unauthorized`] — if `caller` is not the admin.
    /// * [`RouterError::RouteAlreadyExists`] — if a route with `name` already exists.
    /// * [`RouterError::NotInitialized`] — if the contract has not been initialized.
    pub fn register_route(
        env: Env,
        caller: Address,
        name: String,
        address: Address,
        metadata: Option<RouteMetadata>,
    ) -> Result<(), RouterError> {
        caller.require_auth();
        router_common::require_admin_simple!(&env, &caller, &DataKey::Admin, RouterError)?;

        // Use shared validation helper
        Self::validate_route_name(&env, &name)?;

        // Validate metadata if provided
        if let Some(ref meta) = metadata {
            if meta.description.len() > 256 {
                return Err(RouterError::InvalidMetadata);
            }
            if meta.tags.len() > 5 {
                return Err(RouterError::InvalidMetadata);
            }
        }

        let entry = RouteEntry {
            address,
            name: name.clone(),
            paused: false,
            updated_by: caller,
        };
        env.storage()
            .instance()
            .set(&DataKey::Route(name.clone()), &entry);

        if let Some(meta) = metadata {
            env.storage()
                .instance()
                .set(&DataKey::Metadata(name.clone()), &meta);
        }

        let mut route_names = Self::get_route_names(&env);
        route_names.push_back(name.clone());
        env.storage()
            .instance()
            .set(&DataKey::RouteNames, &route_names);

        let count: u32 = env
            .storage()
            .instance()
            .get(&DataKey::RouteCount)
            .unwrap_or(0);
        env.storage()
            .instance()
            .set(&DataKey::RouteCount, &(count + 1));

        env.events().publish(
            (Symbol::new(&env, "route_registered"),),
            (name.clone(), entry.address.clone()),
        );

        Ok(())
    }

    /// Update an existing route to point to a new address.
    ///
    /// Replaces the contract address for an existing route. The route must
    /// already exist. Caller must be the admin. Emits both a `route_updated`
    /// event and a `route_overwritten` event carrying the old and new addresses
    /// so that off-chain observers can detect unintended redirections.
    ///
    /// # Arguments
    /// * `env` - The Soroban environment.
    /// * `caller` - The address initiating the call; must be the admin.
    /// * `name` - The name of the route to update.
    /// * `new_address` - The new contract address for this route.
    ///
    /// # Returns
    /// `Ok(())` on success.
    ///
    /// # Errors
    /// * [`RouterError::Unauthorized`] — if `caller` is not the admin.
    /// * [`RouterError::RouteNotFound`] — if no route with `name` exists.
    /// * [`RouterError::NotInitialized`] — if the contract has not been initialized.
    pub fn update_route(
        env: Env,
        caller: Address,
        name: String,
        new_address: Address,
    ) -> Result<(), RouterError> {
        caller.require_auth();
        router_common::require_admin_simple!(&env, &caller, &DataKey::Admin, RouterError)?;

        let mut entry: RouteEntry = env
            .storage()
            .instance()
            .get(&DataKey::Route(name.clone()))
            .ok_or(RouterError::RouteNotFound)?;

        let old_address = entry.address.clone();
        entry.address = new_address.clone();
        entry.updated_by = caller;
        env.storage()
            .instance()
            .set(&DataKey::Route(name.clone()), &entry);

        env.events()
            .publish((Symbol::new(&env, "route_updated"),), name.clone());

        env.events().publish(
            (Symbol::new(&env, "route_overwritten"),),
            (name.clone(), old_address, new_address),
        );

        Ok(())
    }

    /// Remove a route entirely.
    ///
    /// Deletes the route entry for `name` from storage and removes any aliases 
    /// that point to this route. Caller must be the admin.
    ///
    /// # Arguments
    /// * `env` - The Soroban environment.
    /// * `caller` - The address initiating the call; must be the admin.
    /// * `name` - The name of the route to remove.
    ///
    /// # Returns
    /// `Ok(())` on success.
    ///
    /// # Errors
    /// * [`RouterError::Unauthorized`] — if `caller` is not the admin.
    /// * [`RouterError::RouteNotFound`] — if no route with `name` exists.
    /// * [`RouterError::NotInitialized`] — if the contract has not been initialized.
    pub fn remove_route(env: Env, caller: Address, name: String) -> Result<(), RouterError> {
        caller.require_auth();
        router_common::require_admin_simple!(&env, &caller, &DataKey::Admin, RouterError)?;

        if !env.storage().instance().has(&DataKey::Route(name.clone())) {
            return Err(RouterError::RouteNotFound);
        }

        env.storage()
            .instance()
            .remove(&DataKey::Route(name.clone()));
        env.storage()
            .instance()
            .remove(&DataKey::Metadata(name.clone()));

        let route_names = Self::get_route_names(&env);
        let mut updated_route_names = Vec::new(&env);
        for route_name in route_names.iter() {
            if route_name != name {
                updated_route_names.push_back(route_name);
            }
        }
        env.storage()
            .instance()
            .set(&DataKey::RouteNames, &updated_route_names);

        let count: u32 = env
            .storage()
            .instance()
            .get(&DataKey::RouteCount)
            .unwrap_or(0);
        env.storage()
            .instance()
            .set(&DataKey::RouteCount, &count.saturating_sub(1));

        // Clean up any aliases pointing to this route
        let aliases = Self::get_aliases(&env);
        let mut updated_aliases = Vec::new(&env);
        for alias in aliases.iter() {
            if let Some(original_name) = env.storage().instance().get::<DataKey, String>(&DataKey::Alias(alias.clone())) {
                if original_name == name {
                    // Remove this dangling alias
                    env.storage().instance().remove(&DataKey::Alias(alias.clone()));
                } else {
                    // Keep this alias
                    updated_aliases.push_back(alias);
                }
            } else {
                // Alias doesn't exist in storage, remove from list
                // (this shouldn't happen but cleans up inconsistencies)
            }
        }
        env.storage().instance().set(&DataKey::Aliases, &updated_aliases);

        env.events()
            .publish((Symbol::new(&env, "route_removed"),), name.clone());

        Ok(())
    }

    /// Register multiple routes in a single transaction.
    ///
    /// Associates multiple human-readable names with target contract addresses
    /// in a single atomic operation. All routes start in an unpaused state.
    /// Caller must be the admin. If any route fails validation, the entire
    /// batch fails and no routes are registered.
    ///
    /// # Arguments
    /// * `env` - The Soroban environment.
    /// * `caller` - The address initiating the call; must be the admin.
    /// * `routes` - A vector of tuples (name, address, metadata) for each route to register.
    ///
    /// # Returns
    /// `Ok(())` on success.
    ///
    /// # Errors
    /// * [`RouterError::Unauthorized`] — if `caller` is not the admin.
    /// * [`RouterError::RouteAlreadyExists`] — if any route name already exists.
    /// * [`RouterError::InvalidRouteName`] — if any route name is empty or whitespace-only.
    /// * [`RouterError::InvalidMetadata`] — if any metadata is invalid.
    /// * [`RouterError::NotInitialized`] — if the contract has not been initialized.
    pub fn register_routes_batch(
        env: Env,
        caller: Address,
        routes: Vec<RouteRegisterInput>,
    ) -> Result<(), RouterError> {
        caller.require_auth();
        router_common::require_admin_simple!(&env, &caller, &DataKey::Admin, RouterError)?;

        // Validation phase: check all routes before writing any
        let mut seen = Vec::new(&env);
        for route in routes.iter() {
            if seen.contains(&route.name) {
                return Err(RouterError::RouteAlreadyExists);
            }
            Self::validate_route_name(&env, &route.name)?;

            // Validate metadata if provided
            if let Some(ref meta) = route.metadata {
                if meta.description.len() > 256 {
                    return Err(RouterError::InvalidMetadata);
                }
                if meta.tags.len() > 5 {
                    return Err(RouterError::InvalidMetadata);
                }
            }
            seen.push_back(route.name.clone());
        }

        // Commit phase: write all routes
        let mut route_names = Self::get_route_names(&env);
        for route in routes.iter() {
            let entry = RouteEntry {
                address: route.address.clone(),
                name: route.name.clone(),
                paused: false,
                updated_by: caller.clone(),
                metadata: route.metadata.clone(),
            };
            env.storage()
                .instance()
                .set(&DataKey::Route(route.name.clone()), &entry);

            route_names.push_back(route.name.clone());

            env.events().publish(
                (Symbol::new(&env, "route_registered"),),
                (route.name.clone(), entry.address.clone()),
            );
        }

        env.storage()
            .instance()
            .set(&DataKey::RouteNames, &route_names);

        Ok(())
    }

    /// Remove multiple routes in a single transaction.
    ///
    /// Deletes route entries for all specified names from storage and removes
    /// any aliases that point to these routes. Caller must be the admin.
    /// If any route is not found, the entire batch fails and no routes are removed.
    ///
    /// # Arguments
    /// * `env` - The Soroban environment.
    /// * `caller` - The address initiating the call; must be the admin.
    /// * `names` - A vector of route names to remove.
    ///
    /// # Returns
    /// `Ok(())` on success.
    ///
    /// # Errors
    /// * [`RouterError::Unauthorized`] — if `caller` is not the admin.
    /// * [`RouterError::RouteNotFound`] — if any route name does not exist.
    /// * [`RouterError::NotInitialized`] — if the contract has not been initialized.
    pub fn remove_routes_batch(
        env: Env,
        caller: Address,
        names: Vec<String>,
    ) -> Result<(), RouterError> {
        caller.require_auth();
        router_common::require_admin_simple!(&env, &caller, &DataKey::Admin, RouterError)?;

        // Validation phase: check all routes exist
        for name in names.iter() {
            if !env.storage().instance().has(&DataKey::Route(name.clone())) {
                return Err(RouterError::RouteNotFound);
            }
        }

        // Commit phase: remove all routes
        let route_names = Self::get_route_names(&env);
        let mut updated_route_names = Vec::new(&env);
        for route_name in route_names.iter() {
            if !names.contains(&route_name) {
                updated_route_names.push_back(route_name);
            }
        }
        env.storage()
            .instance()
            .set(&DataKey::RouteNames, &updated_route_names);

        // Clean up aliases pointing to removed routes
        let aliases = Self::get_aliases(&env);
        let mut updated_aliases = Vec::new(&env);
        for alias in aliases.iter() {
            if let Some(original_name) = env.storage().instance().get::<DataKey, String>(&DataKey::Alias(alias.clone())) {
                if names.contains(&original_name) {
                    // Remove this dangling alias
                    env.storage().instance().remove(&DataKey::Alias(alias.clone()));
                } else {
                    // Keep this alias
                    updated_aliases.push_back(alias);
                }
            }
        }
        env.storage().instance().set(&DataKey::Aliases, &updated_aliases);

        // Remove route entries and emit events
        for name in names.iter() {
            env.storage()
                .instance()
                .remove(&DataKey::Route(name.clone()));

            env.events()
                .publish((Symbol::new(&env, "route_removed"),), name.clone());
        }

        Ok(())
    }

    /// Resolve a route name to its contract address.
    ///
    /// Looks up the contract address registered under `name`, validates that
    /// neither the router nor the individual route is paused, increments the
    /// total-routed counter, and emits a `routed` event. If `name` is an alias,
    /// resolves to the original route.
    ///
    /// When multiple scored routes exist, score-based selection is applied:
    /// all route names are evaluated via `get_best_route` and the highest-scoring
    /// non-paused route is returned automatically. If no scored routes exist,
    /// falls back to the direct lookup by `name`.
    ///
    /// # Arguments
    /// * `env` - The Soroban environment.
    /// * `name` - The name of the route to resolve.
    ///
    /// # Returns
    /// The [`Address`] of the contract registered under `name`.
    ///
    /// # Errors
    /// * [`RouterError::RouterPaused`] — if the entire router is paused.
    /// * [`RouterError::RouteNotFound`] — if no route with `name` exists.
    /// * [`RouterError::RoutePaused`] — if the specific route is paused.
    pub fn resolve(env: Env, name: String) -> Result<Address, RouterError> {
        let paused: bool = env
            .storage()
            .instance()
            .get(&DataKey::Paused)
            .unwrap_or(false);
        if paused {
            return Err(RouterError::RouterPaused);
        }

        // Resolve alias if present
        let resolved_name = if let Some(original) = env
            .storage()
            .instance()
            .get::<DataKey, String>(&DataKey::Alias(name.clone()))
        {
            original
        } else {
            name.clone()
        };

        // Score-based selection: if any route has a score, use get_best_route
        // across all candidates to return the highest-scoring non-paused route.
        let all_names = Self::get_route_names(&env);
        let has_any_score = all_names
            .iter()
            .any(|n| env.storage().instance().has(&DataKey::Score(n.clone())));

        let final_name = if has_any_score {
            // Use score-based selection over all routes; fall back to resolved_name
            match Self::get_best_route(
                env.clone(),
                all_names,
                i64::MIN,
                Some(resolved_name.clone()),
            )? {
                Some(best) => best,
                None => resolved_name.clone(),
            }
        } else {
            resolved_name.clone()
        };

        let entry: RouteEntry = env
            .storage()
            .instance()
            .get(&DataKey::Route(final_name.clone()))
            .ok_or(RouterError::RouteNotFound)?;

        if entry.paused {
            env.events().publish(
                (Symbol::new(&env, "route_resolve_paused"),),
                (final_name.clone(),),
            );
            return Err(RouterError::RoutePaused);
        }

        // Increment total routed counter
        let total: u64 = env
            .storage()
            .instance()
            .get(&DataKey::TotalRouted)
            .unwrap_or(0);
        env.storage()
            .instance()
            .set(&DataKey::TotalRouted, &(total + 1));

        env.events().publish(
            (Symbol::new(&env, "routed"),),
            (name.clone(), entry.address.clone()),
        );

        Ok(entry.address)
    }

    /// Pause or unpause a specific route.
    ///
    /// When a route is paused, calls to `resolve` for that route will
    /// return [`RouterError::RoutePaused`]. Caller must be the admin.
    ///
    /// # Arguments
    /// * `env` - The Soroban environment.
    /// * `caller` - The address initiating the call; must be the admin.
    /// * `name` - The name of the route to pause or unpause.
    /// * `paused` - `true` to pause the route, `false` to unpause it.
    ///
    /// # Returns
    /// `Ok(())` on success.
    ///
    /// # Errors
    /// * [`RouterError::Unauthorized`] — if `caller` is not the admin.
    /// * [`RouterError::RouteNotFound`] — if no route with `name` exists.
    /// * [`RouterError::NotInitialized`] — if the contract has not been initialized.
    pub fn set_route_paused(
        env: Env,
        caller: Address,
        name: String,
        paused: bool,
    ) -> Result<(), RouterError> {
        caller.require_auth();
        router_common::require_admin_simple!(&env, &caller, &DataKey::Admin, RouterError)?;

        let mut entry: RouteEntry = env
            .storage()
            .instance()
            .get(&DataKey::Route(name.clone()))
            .ok_or(RouterError::RouteNotFound)?;

        entry.paused = paused;
        entry.updated_by = caller.clone();
        env.storage()
            .instance()
            .set(&DataKey::Route(name.clone()), &entry);

        env.events()
            .publish((Symbol::new(&env, "route_paused"),), (name.clone(), paused));

        Ok(())
    }

    /// Pause or unpause the entire router.
    ///
    /// When the router is paused, all calls to `resolve` will return
    /// [`RouterError::RouterPaused`] regardless of individual route state.
    /// Caller must be the admin.
    ///
    /// # Arguments
    /// * `env` - The Soroban environment.
    /// * `caller` - The address initiating the call; must be the admin.
    /// * `paused` - `true` to pause the router, `false` to unpause it.
    ///
    /// # Returns
    /// `Ok(())` on success.
    ///
    /// # Errors
    /// * [`RouterError::Unauthorized`] — if `caller` is not the admin.
    /// * [`RouterError::NotInitialized`] — if the contract has not been initialized.
    pub fn set_paused(env: Env, caller: Address, paused: bool) -> Result<(), RouterError> {
        caller.require_auth();
        router_common::require_admin_simple!(&env, &caller, &DataKey::Admin, RouterError)?;
        env.storage().instance().set(&DataKey::Paused, &paused);

        env.events()
            .publish((Symbol::new(&env, "router_paused"),), paused);

        Ok(())
    }

    /// Get a route entry by name.
    ///
    /// Returns the full [`RouteEntry`] for the given `name`, or `None` if no
    /// such route is registered.
    ///
    /// # Arguments
    /// * `env` - The Soroban environment.
    /// * `name` - The name of the route to look up.
    ///
    /// # Returns
    /// `Some(`[`RouteEntry`]`)` if the route exists, `None` otherwise.
    pub fn get_route(env: Env, name: String) -> Option<RouteEntry> {
        env.storage().instance().get(&DataKey::Route(name))
    }

    /// Update metadata for an existing route.
    ///
    /// Allows updating route metadata independently of the route address.
    /// Caller must be the admin.
    ///
    /// # Arguments
    /// * `env` - The Soroban environment.
    /// * `caller` - The address initiating the call; must be the admin.
    /// * `name` - The name of the route to update.
    /// * `metadata` - The new metadata for the route.
    ///
    /// # Returns
    /// `Ok(())` on success.
    ///
    /// # Errors
    /// * [`RouterError::Unauthorized`] — if `caller` is not the admin.
    /// * [`RouterError::RouteNotFound`] — if no route with `name` exists.
    pub fn update_metadata(
        env: Env,
        caller: Address,
        name: String,
        metadata: Option<RouteMetadata>,
    ) -> Result<(), RouterError> {
        caller.require_auth();
        router_common::require_admin_simple!(&env, &caller, &DataKey::Admin, RouterError)?;

        if !env.storage().instance().has(&DataKey::Route(name.clone())) {
            return Err(RouterError::RouteNotFound);
        }

        // Validate metadata if provided
        if let Some(ref meta) = metadata {
            if meta.description.len() > 256 {
                return Err(RouterError::InvalidMetadata);
            }
            if meta.tags.len() > 5 {
                return Err(RouterError::InvalidMetadata);
            }
        }

        match metadata.clone() {
            Some(meta) => env
                .storage()
                .instance()
                .set(&DataKey::Metadata(name.clone()), &meta),
            None => env
                .storage()
                .instance()
                .remove(&DataKey::Metadata(name.clone())),
        }

        env.events().publish(
            (Symbol::new(&env, "metadata_updated"),),
            (name.clone(), metadata.is_some()),
        );

        Ok(())
    }

    /// Get metadata for a route.
    ///
    /// Returns the metadata for the given route name, or `None` if no
    /// metadata is set or the route doesn't exist.
    ///
    /// # Arguments
    /// * `env` - The Soroban environment.
    /// * `name` - The name of the route.
    ///
    /// # Returns
    /// `Some(`[`RouteMetadata`]`)` if metadata exists, `None` otherwise.
    pub fn get_metadata(env: Env, name: String) -> Option<RouteMetadata> {
        env.storage()
            .instance()
            .get::<DataKey, RouteMetadata>(&DataKey::Metadata(name))
    }

    /// Get the total number of resolved calls.
    ///
    /// Returns the cumulative count of successful `resolve` invocations
    /// since the contract was initialized.
    ///
    /// # Arguments
    /// * `env` - The Soroban environment.
    ///
    /// # Returns
    /// The total number of times a route has been resolved.
    pub fn total_routed(env: Env) -> u64 {
        env.storage()
            .instance()
            .get(&DataKey::TotalRouted)
            .unwrap_or(0)
    }

    /// Get the total number of registered routes.
    ///
    /// Returns the count of all registered routes (excluding aliases).
    ///
    /// # Arguments
    /// * `env` - The Soroban environment.
    ///
    /// # Returns
    /// The total number of registered routes.
    pub fn route_count(env: Env) -> u32 {
        env.storage()
            .instance()
            .get(&DataKey::RouteCount)
            .unwrap_or(0)
    }

    /// Create an alias for an existing route.
    ///
    /// Associates `alias_name` with the same address as `existing_name`.
    /// When `alias_name` is resolved, it returns the address of `existing_name`.
    /// Caller must be the admin.
    ///
    /// # Arguments
    /// * `env` - The Soroban environment.
    /// * `caller` - The address initiating the call; must be the admin.
    /// * `existing_name` - The name of the route to alias.
    /// * `alias_name` - The new alias name.
    ///
    /// # Returns
    /// `Ok(())` on success.
    ///
    /// # Errors
    /// * [`RouterError::Unauthorized`] — if `caller` is not the admin.
    /// * [`RouterError::RouteNotFound`] — if `existing_name` does not exist.
    /// * [`RouterError::RouteAlreadyExists`] — if `alias_name` already exists.
    pub fn add_alias(
        env: Env,
        caller: Address,
        existing_name: String,
        alias_name: String,
    ) -> Result<(), RouterError> {
        caller.require_auth();
        router_common::require_admin_simple!(&env, &caller, &DataKey::Admin, RouterError)?;

        // Verify existing route exists
        if !env
            .storage()
            .instance()
            .has(&DataKey::Route(existing_name.clone()))
        {
            return Err(RouterError::RouteNotFound);
        }

        // Use shared validation helper for alias name
        Self::validate_route_name(&env, &alias_name)?;

        env.storage()
            .instance()
            .set(&DataKey::Alias(alias_name.clone()), &existing_name);

        // Track alias name for cleanup
        let mut aliases = Self::get_aliases(&env);
        if !aliases.contains(&alias_name) {
            aliases.push_back(alias_name.clone());
            env.storage().instance().set(&DataKey::Aliases, &aliases);
        }

        env.events().publish(
            (Symbol::new(&env, "alias_added"),),
            (existing_name, alias_name),
        );

        Ok(())
    }

    /// Remove an alias.
    ///
    /// Deletes the alias mapping for `alias_name`. Caller must be the admin.
    ///
    /// # Arguments
    /// * `env` - The Soroban environment.
    /// * `caller` - The address initiating the call; must be the admin.
    /// * `alias_name` - The alias to remove.
    ///
    /// # Returns
    /// `Ok(())` on success.
    ///
    /// # Errors
    /// * [`RouterError::Unauthorized`] — if `caller` is not the admin.
    /// * [`RouterError::RouteNotFound`] — if `alias_name` does not exist.
    pub fn remove_alias(env: Env, caller: Address, alias_name: String) -> Result<(), RouterError> {
        caller.require_auth();
        router_common::require_admin_simple!(&env, &caller, &DataKey::Admin, RouterError)?;

        if !env
            .storage()
            .instance()
            .has(&DataKey::Alias(alias_name.clone()))
        {
            return Err(RouterError::RouteNotFound);
        }
        env.storage()
            .instance()
            .remove(&DataKey::Alias(alias_name.clone()));

        // Remove from aliases list
        let aliases = Self::get_aliases(&env);
        let mut updated_aliases = Vec::new(&env);
        for alias in aliases.iter() {
            if alias != alias_name {
                updated_aliases.push_back(alias);
            }
        }
        env.storage().instance().set(&DataKey::Aliases, &updated_aliases);

        env.events()
            .publish((Symbol::new(&env, "alias_removed"),), alias_name);

        Ok(())
    }

    /// Get current admin.
    ///
    /// # Arguments
    /// * `env` - The Soroban environment.
    ///
    /// # Returns
    /// The [`Address`] of the current admin.
    ///
    /// # Panics
    /// * Panics if the contract has not been initialized.
    /// 
    /// Note: This is a breaking change from the previous Result-based API.
    /// Calling admin() on an uninitialized contract is considered a programming error
    /// rather than a runtime condition, consistent with how total_routed() works.
    pub fn admin(env: Env) -> Address {
        env.storage()
            .instance()
            .get(&DataKey::Admin)
            .expect("not initialized")
    }

    /// Transfer admin to a new address.
    ///
    /// Replaces the current admin with `new_admin`. The `current` address must
    /// authenticate and must be the existing admin.
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
    /// * [`RouterError::Unauthorized`] — if `current` is not the admin.
    /// * [`RouterError::NotInitialized`] — if the contract has not been initialized.
    pub fn transfer_admin(
        env: Env,
        current: Address,
        new_admin: Address,
    ) -> Result<(), RouterError> {
        current.require_auth();
        router_common::require_admin_simple!(&env, &current, &DataKey::Admin, RouterError)?;
        router_common::admin_transfer_complete!(&env, &current, &new_admin, &DataKey::Admin);
        Ok(())
    }

    /// Returns all currently registered route names as a vector of strings.
    ///
    /// This is a read-only operation. The order of returned names is not guaranteed.
    ///
    /// # Arguments
    /// * `env` - The Soroban environment.
    ///
    /// # Returns
    /// A `Vec<String>` containing all registered route names.
    pub fn get_all_routes(env: Env) -> Vec<String> {
        Self::get_route_names(&env)
    }

    /// Returns the canonical route name that `alias_name` points to, or `None`.
    ///
    /// This is a read-only lookup that does not increment `total_routed` and
    /// does not fail when the router or route is paused.
    ///
    /// # Arguments
    /// * `env` - The Soroban environment.
    /// * `alias_name` - The alias name to look up.
    ///
    /// # Returns
    /// `Some(canonical_name)` if `alias_name` is a registered alias, `None` otherwise.
    pub fn get_alias_target(env: Env, alias_name: String) -> Option<String> {
        env.storage()
            .instance()
            .get::<DataKey, String>(&DataKey::Alias(alias_name))
    }

    /// Set or update the scoring attributes for a route.
    ///
    /// Scores are used by [`get_best_route`] to select the optimal path from a
    /// set of candidates. Caller must be the admin.
    ///
    /// # Arguments
    /// * `env` - The Soroban environment.
    /// * `caller` - Must be the admin.
    /// * `name` - The route to score.
    /// * `score` - The [`RouteScore`] to associate with this route.
    ///
    /// # Errors
    /// * [`RouterError::Unauthorized`] — if `caller` is not the admin.
    /// * [`RouterError::RouteNotFound`] — if the route does not exist.
    /// * [`RouterError::NotInitialized`] — if the contract is not initialized.
    pub fn set_route_score(
        env: Env,
        caller: Address,
        name: String,
        score: RouteScore,
    ) -> Result<(), RouterError> {
        caller.require_auth();
        router_common::require_admin_simple!(&env, &caller, &DataKey::Admin, RouterError)?;

        if !env.storage().instance().has(&DataKey::Route(name.clone())) {
            return Err(RouterError::RouteNotFound);
        }

        env.storage()
            .instance()
            .set(&DataKey::Score(name.clone()), &score);

        env.events().publish(
            (Symbol::new(&env, "route_scored"),),
            (name, score.liquidity_score, score.fee_bps, score.reliability_score),
        );

        Ok(())
    }

    /// Get the score for a route.
    ///
    /// Returns `None` if no score has been set for the route.
    pub fn get_route_score(env: Env, name: String) -> Option<RouteScore> {
        env.storage().instance().get(&DataKey::Score(name))
    }

    /// Select the best route from a list of candidates.
    ///
    /// Evaluates each candidate route using a composite score:
    /// `liquidity_score + reliability_score - fee_bps / 10`
    ///
    /// Routes that are paused or have no score are skipped. Returns the name
    /// of the highest-scoring available route, or `fallback_name` if no
    /// candidate meets `min_score`.
    ///
    /// # Arguments
    /// * `env` - The Soroban environment.
    /// * `candidates` - A list of route names to evaluate.
    /// * `min_score` - Minimum composite score a route must reach to be selected.
    /// * `fallback_name` - Returned when no candidate meets `min_score`.
    ///
    /// # Returns
    /// The name of the best route, `fallback_name` if none meet the threshold,
    /// or `None` if no scoreable, unpaused route exists and no fallback is set.
    ///
    /// # Errors
    /// * [`RouterError::RouterPaused`] — if the entire router is paused.
    pub fn get_best_route(env: Env, candidates: Vec<String>, min_score: i64, fallback_name: Option<String>) -> Result<Option<String>, RouterError> {
        let paused: bool = env
            .storage()
            .instance()
            .get(&DataKey::Paused)
            .unwrap_or(false);
        if paused {
            return Err(RouterError::RouterPaused);
        }

        let mut best_name: Option<String> = None;
        let mut best_score: i64 = i64::MIN;

        for name in candidates.iter() {
            // Skip paused routes
            let entry: Option<RouteEntry> = env
                .storage()
                .instance()
                .get(&DataKey::Route(name.clone()));
            let entry = match entry {
                Some(e) if !e.paused => e,
                _ => continue,
            };
            let _ = entry; // entry validated, not needed further

            // Skip routes without a score
            let score: RouteScore = match env
                .storage()
                .instance()
                .get(&DataKey::Score(name.clone()))
            {
                Some(s) => s,
                None => continue,
            };

            // Composite score: liquidity + reliability - fee_bps/10
            let composite: i64 = score.liquidity_score as i64
                + score.reliability_score as i64
                - (score.fee_bps as i64 / 10);

            if composite > best_score {
                best_score = composite;
                best_name = Some(name.clone());
            }
        }

        // Apply minimum score threshold: fall back if best doesn't meet it
        let result = if best_score >= min_score {
            if let Some(ref name) = best_name {
                env.events().publish(
                    (Symbol::new(&env, "best_route_selected"),),
                    (name.clone(), best_score),
                );
            }
            best_name
        } else {
            fallback_name
        };

        Ok(result)
    }

    /// Resolve multiple route names in a single call.
    ///
    /// Resolves each name in `names` using the same logic as [`resolve`],
    /// returning one [`BatchResolveResult`] per input name in the same order.
    /// Clients that need multiple addresses should prefer this over repeated
    /// `resolve` calls to avoid extra round-trips.
    ///
    /// # Arguments
    /// * `env` - The Soroban environment.
    /// * `names` - Route names (or aliases) to resolve.
    ///
    /// # Returns
    /// A `Vec<BatchResolveResult>` with one entry per input name, preserving order.
    pub fn batch_resolve(env: Env, names: Vec<String>) -> Vec<BatchResolveResult> {
        let mut results = Vec::new(&env);
        for name in names.iter() {
            let outcome = match Self::resolve(env.clone(), name) {
                Ok(addr) => BatchResolveResult::Ok(addr),
                Err(RouterError::RouterPaused) => {
                    BatchResolveResult::Err(ResolveError::RouterPaused)
                }
                Err(RouterError::RoutePaused) => BatchResolveResult::Err(ResolveError::RoutePaused),
                Err(_) => BatchResolveResult::Err(ResolveError::RouteNotFound),
            };
            results.push_back(outcome);
        }
        results
    }

    // ── Helpers ───────────────────────────────────────────────────────────────

    fn get_route_names(env: &Env) -> Vec<String> {
        env.storage()
            .instance()
            .get(&DataKey::RouteNames)
            .unwrap_or(Vec::new(env))
    }

    fn get_aliases(env: &Env) -> Vec<String> {
        env.storage()
            .instance()
            .get(&DataKey::Aliases)
            .unwrap_or(Vec::new(env))
    }

    /// Returns `true` if `name` is empty or consists entirely of ASCII whitespace
    /// characters (space 0x20, tab 0x09, newline 0x0A, vertical tab 0x0B,
    /// form feed 0x0C, carriage return 0x0D).
    fn is_empty_or_whitespace(name: &String) -> bool {
        if name.len() == 0 {
            return true;
        }
        let s = name.to_string();
        s.bytes().all(|b| matches!(b, 9 | 10 | 11 | 12 | 13 | 32))
    }

    /// Validates a route name for use in register_route and add_alias.
    ///
    /// Valid names are 1–64 characters, containing only ASCII alphanumeric
    /// characters, hyphens (`-`), and forward slashes (`/`).
    ///
    /// # Arguments
    /// * `env` - The Soroban environment.
    /// * `name` - The route or alias name to validate.
    ///
    /// # Returns
    /// `Ok(())` if the name is valid and available.
    ///
    /// # Errors
    /// * [`RouterError::InvalidRouteName`] — if the name is empty, whitespace-only, longer than 64 chars, or contains disallowed characters.
    /// * [`RouterError::RouteAlreadyExists`] — if the name conflicts with an existing route or alias.
    fn validate_route_name(env: &Env, name: &String) -> Result<(), RouterError> {
        if Self::is_empty_or_whitespace(name) {
            return Err(RouterError::InvalidRouteName);
        }

        // Max 64 characters
        if name.len() > 64 {
            return Err(RouterError::InvalidRouteName);
        }

        // Only alphanumeric, '-', and '/' are allowed
        let s = name.to_string();
        for b in s.bytes() {
            if !matches!(b, b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'-' | b'/') {
                return Err(RouterError::InvalidRouteName);
            }
        }

        // Check if name already exists as a route
        if env.storage().instance().has(&DataKey::Route(name.clone())) {
            return Err(RouterError::RouteAlreadyExists);
        }

        // Check if name already exists as an alias
        if env.storage().instance().has(&DataKey::Alias(name.clone())) {
            return Err(RouterError::RouteAlreadyExists);
        }

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
        vec, Env, IntoVal, String,
    };

    fn setup() -> (Env, Address, RouterCoreClient<'static>) {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register_contract(None, RouterCore);
        let client = RouterCoreClient::new(&env, &contract_id);
        let admin = Address::generate(&env);
        client.initialize(&admin);
        (env, admin, client)
    }

    #[test]
    fn test_register_and_resolve() {
        let (env, admin, client) = setup();
        let name = String::from_str(&env, "oracle");
        let addr = Address::generate(&env);
        client.register_route(&admin, &name, &addr, &None);
        let resolved = client.resolve(&name);
        assert_eq!(resolved, addr);
        assert_eq!(client.total_routed(), 1);

        // Verify route_registered event carries both name and address
        let events = env.events().all();
        let reg_event = events
            .iter()
            .find(|e| {
                e.1.get(0)
                    .map(|v| {
                        let s: Symbol = v.into_val(&env);
                        s == Symbol::new(&env, "route_registered")
                    })
                    .unwrap_or(false)
            })
            .unwrap();
        let (emitted_name, emitted_addr): (String, Address) = reg_event.2.into_val(&env);
        assert_eq!(emitted_name, name);
        assert_eq!(emitted_addr, addr);
    }

    #[test]
    fn test_update_route() {
        let (env, admin, client) = setup();
        let name = String::from_str(&env, "oracle");
        let addr1 = Address::generate(&env);
        let addr2 = Address::generate(&env);
        client.register_route(&admin, &name, &addr1, &None);
        client.update_route(&admin, &name, &addr2);
        assert_eq!(client.resolve(&name), addr2);
    }

    #[test]
    fn test_remove_route() {
        let (env, admin, client) = setup();
        let name = String::from_str(&env, "oracle");
        let addr = Address::generate(&env);
        client.register_route(&admin, &name, &addr, &None);
        client.remove_route(&admin, &name);
        let result = client.try_resolve(&name);
        assert_eq!(result, Err(Ok(RouterError::RouteNotFound)));
    }

    #[test]
    fn test_duplicate_route_fails() {
        let (env, admin, client) = setup();
        let name = String::from_str(&env, "oracle");
        let addr = Address::generate(&env);
        client.register_route(&admin, &name, &addr, &None);
        let result = client.try_register_route(&admin, &name, &addr, &None);
        assert_eq!(result, Err(Ok(RouterError::RouteAlreadyExists)));
    }

    #[test]
    fn test_register_route_description_too_long_returns_invalid_metadata() {
        let (env, admin, client) = setup();
        let name = String::from_str(&env, "oracle");
        let addr = Address::generate(&env);
        let long_description = alloc::string::String::from("a").repeat(257);
        let metadata = RouteMetadata {
            description: String::from_str(&env, &long_description),
            tags: Vec::new(&env),
            owner: admin.clone(),
        };

        let result = client.try_register_route(&admin, &name, &addr, &Some(metadata));
        assert_eq!(result, Err(Ok(RouterError::InvalidMetadata)));
    }

    #[test]
    fn test_register_route_too_many_tags_returns_invalid_metadata() {
        let (env, admin, client) = setup();
        let name = String::from_str(&env, "oracle");
        let addr = Address::generate(&env);
        let mut tags = Vec::new(&env);
        for i in 0..6 {
            tags.push_back(String::from_str(&env, &alloc::string::String::from("tag").repeat(i + 1)));
        }
        let metadata = RouteMetadata {
            description: String::from_str(&env, "valid description"),
            tags,
            owner: admin.clone(),
        };

        let result = client.try_register_route(&admin, &name, &addr, &Some(metadata));
        assert_eq!(result, Err(Ok(RouterError::InvalidMetadata)));
    }

    #[test]
    fn test_pause_route() {
        let (env, admin, client) = setup();
        let name = String::from_str(&env, "oracle");
        let addr = Address::generate(&env);
        client.register_route(&admin, &name, &addr, &None);
        client.set_route_paused(&admin, &name, &true);
        let result = client.try_resolve(&name);
        assert_eq!(result, Err(Ok(RouterError::RoutePaused)));
    }

    #[test]
    fn test_pause_and_unpause_route() {
        let (env, admin, client) = setup();
        let name = String::from_str(&env, "oracle");
        let addr = Address::generate(&env);

        // Register a route
        client.register_route(&admin, &name, &addr, &None);

        // Verify resolve works initially
        let resolved = client.resolve(&name);
        assert_eq!(resolved, addr);

        // Pause the route
        client.set_route_paused(&admin, &name, &true);

        // Assert that resolve now fails with RoutePaused error
        let result = client.try_resolve(&name);
        assert_eq!(result, Err(Ok(RouterError::RoutePaused)));

        // Unpause the route
        client.set_route_paused(&admin, &name, &false);

        // Assert that resolve works again
        let resolved = client.resolve(&name);
        assert_eq!(resolved, addr);
    }

    #[test]
    fn test_paused_route_emits_event() {
        let (env, admin, client) = setup();
        let name = String::from_str(&env, "oracle");
        let addr = Address::generate(&env);

        client.register_route(&admin, &name, &addr, &None);
        client.set_route_paused(&admin, &name, &true);

        // Attempt to resolve the paused route
        let _ = client.try_resolve(&name);

        // Verify the route_resolve_paused event was emitted
        let event = env.events().all().last().unwrap().clone();
        assert_eq!(event.0, client.address);
        assert_eq!(
            event.1,
            vec![
                &env,
                Symbol::new(&env, "route_resolve_paused").into_val(&env)
            ]
        );
        let (emitted_name,): (String,) = event.2.into_val(&env);
        assert_eq!(emitted_name, name);
    }

    #[test]
    fn test_pause_router() {
        let (env, admin, client) = setup();
        let name = String::from_str(&env, "oracle");
        let addr = Address::generate(&env);
        client.register_route(&admin, &name, &addr, &None);
        client.set_paused(&admin, &true);
        let result = client.try_resolve(&name);
        assert_eq!(result, Err(Ok(RouterError::RouterPaused)));
    }

    #[test]
    fn test_unauthorized_register_fails() {
        let (env, _admin, client) = setup();
        let attacker = Address::generate(&env);
        let name = String::from_str(&env, "oracle");
        let addr = Address::generate(&env);
        let result = client.try_register_route(&attacker, &name, &addr, &None);
        assert_eq!(result, Err(Ok(RouterError::Unauthorized)));
    }

    #[test]
    fn test_transfer_admin() {
        let (env, admin, client) = setup();
        let new_admin = Address::generate(&env);
        client.transfer_admin(&admin, &new_admin);
        assert_eq!(client.admin(), new_admin);
    }

    #[test]
    fn test_register_empty_name_fails() {
        let (env, admin, client) = setup();
        let addr = Address::generate(&env);
        let result = client.try_register_route(&admin, &String::from_str(&env, ""), &addr, &None);
        assert_eq!(result, Err(Ok(RouterError::InvalidRouteName)));
    }

    #[test]
    fn test_register_space_only_name_fails() {
        let (env, admin, client) = setup();
        let addr = Address::generate(&env);
        let result = client.try_register_route(&admin, &String::from_str(&env, "   "), &addr, &None);
        assert_eq!(result, Err(Ok(RouterError::InvalidRouteName)));
    }

    #[test]
    fn test_register_tab_only_name_fails() {
        let (env, admin, client) = setup();
        let addr = Address::generate(&env);
        let result = client.try_register_route(&admin, &String::from_str(&env, "\t"), &addr, &None);
        assert_eq!(result, Err(Ok(RouterError::InvalidRouteName)));
    }

    #[test]
    fn test_register_newline_only_name_fails() {
        let (env, admin, client) = setup();
        let addr = Address::generate(&env);
        let result = client.try_register_route(&admin, &String::from_str(&env, "\n"), &addr, &None);
        assert_eq!(result, Err(Ok(RouterError::InvalidRouteName)));
    }

    #[test]
    fn test_transfer_admin_emits_event() {
        let (env, admin, client) = setup();
        let new_admin = Address::generate(&env);

        client.transfer_admin(&admin, &new_admin);

        let event = env.events().all().last().unwrap().clone();
        assert_eq!(event.0, client.address);
        assert_eq!(
            event.1,
            vec![&env, Symbol::new(&env, "admin_transferred").into_val(&env)]
        );
    }

    #[test]
    fn test_set_route_paused_emits_event() {
        let (env, admin, client) = setup();
        let name = String::from_str(&env, "oracle");
        let addr = Address::generate(&env);
        client.register_route(&admin, &name, &addr, &None);

        let events_before = env.events().all().len();
        client.set_route_paused(&admin, &name, &true);
        let events_after = env.events().all().len();

        // Verify an event was emitted
        assert_eq!(events_after, events_before + 1);
    }

    #[test]
    fn test_set_paused_emits_event() {
        let (env, admin, client) = setup();

        let events_before = env.events().all().len();
        client.set_paused(&admin, &true);
        let events_after = env.events().all().len();

        // Verify an event was emitted
        assert_eq!(events_after, events_before + 1);
    }

    #[test]
    fn test_resolve_unknown_route_fails() {
        let (env, _admin, client) = setup();
        let name = String::from_str(&env, "unknown");
        let result = client.try_resolve(&name);
        assert_eq!(result, Err(Ok(RouterError::RouteNotFound)));
    }

    #[test]
    fn test_get_all_routes_empty() {
        let (env, _, client) = setup();
        let routes: Vec<String> = client.get_all_routes();
        assert!(routes.is_empty());
    }

    #[test]
    fn test_initialize_twice_fails() {
        let (env, _, client) = setup();
        let second_admin = Address::generate(&env);
        let result = client.try_initialize(&second_admin);
        assert_eq!(result, Err(Ok(RouterError::AlreadyInitialized)));
    }

    #[test]
    fn test_update_route_while_paused_succeeds() {
        let (env, admin, client) = setup();
        let name = String::from_str(&env, "oracle");
        let addr1 = Address::generate(&env);
        let addr2 = Address::generate(&env);
        client.register_route(&admin, &name, &addr1, &None);
        client.set_route_paused(&admin, &name, &true);
        client.update_route(&admin, &name, &addr2);
        let entry = client.get_route(&name).unwrap();
        assert_eq!(entry.address, addr2);
        assert!(entry.paused); // still paused after update
    }

    #[test]
    fn test_resolve_succeeds_after_unpause() {
        let (env, admin, client) = setup();
        let name = String::from_str(&env, "oracle");
        let addr = Address::generate(&env);
        client.register_route(&admin, &name, &addr, &None);
        client.set_route_paused(&admin, &name, &true);
        client.set_route_paused(&admin, &name, &false);
        assert_eq!(client.resolve(&name), addr);
    }

    #[test]
    fn test_router_unpause_round_trip() {
        let (env, admin, client) = setup();
        let name = String::from_str(&env, "oracle");
        let addr = Address::generate(&env);
        client.register_route(&admin, &name, &addr, &None);
        client.set_paused(&admin, &true);
        assert_eq!(
            client.try_resolve(&name),
            Err(Ok(RouterError::RouterPaused))
        );
        client.set_paused(&admin, &false);
        assert_eq!(client.resolve(&name), addr);
    }

    #[test]
    fn test_update_route_emits_overwritten_event() {
        let (env, admin, client) = setup();
        let name = String::from_str(&env, "oracle");
        let addr1 = Address::generate(&env);
        let addr2 = Address::generate(&env);
        client.register_route(&admin, &name, &addr1, &None);

        let events_before = env.events().all().len();
        client.update_route(&admin, &name, &addr2);
        let events_after = env.events().all().len();

        // Two events: route_updated + route_overwritten
        assert_eq!(events_after, events_before + 2);

        // Verify route_overwritten event carries old and new addresses
        let overwrite_event = env.events().all().last().unwrap().clone();
        assert_eq!(overwrite_event.0, client.address);
        assert_eq!(
            overwrite_event.1,
            vec![&env, Symbol::new(&env, "route_overwritten").into_val(&env)]
        );
    }

    #[test]
    fn test_get_all_routes_multiple() {
        let (env, admin, client) = setup();
        let oracle = String::from_str(&env, "oracle");
        let vault = String::from_str(&env, "vault");
        let addr1 = Address::generate(&env);
        let addr2 = Address::generate(&env);
        client.register_route(&admin, &oracle, &addr1, &None);
        client.register_route(&admin, &vault, &addr2, &None);
        let routes: Vec<String> = client.get_all_routes();
        assert_eq!(routes.len(), 2);
        assert!(routes.contains(&oracle));
        assert!(routes.contains(&vault));
    }

    #[test]
    fn test_register_empty_route_name_fails() {
        let (env, admin, client) = setup();
        let empty_name = String::from_str(&env, "");
        let addr = Address::generate(&env);
        let result = client.try_register_route(&admin, &empty_name, &addr, &None);
        assert_eq!(result, Err(Ok(RouterError::InvalidRouteName)));
    }

    #[test]
    fn test_register_carriage_return_only_name_fails() {
        let (env, admin, client) = setup();
        let addr = Address::generate(&env);
        let result = client.try_register_route(&admin, &String::from_str(&env, "\r"), &addr, &None);
        assert_eq!(result, Err(Ok(RouterError::InvalidRouteName)));
    }

    #[test]
    fn test_register_mixed_whitespace_name_fails() {
        let (env, admin, client) = setup();
        let addr = Address::generate(&env);
        let result = client.try_register_route(&admin, &String::from_str(&env, " \t\n\r"), &addr, &None);
        assert_eq!(result, Err(Ok(RouterError::InvalidRouteName)));
    }

    #[test]
    fn test_register_name_too_long_fails() {
        let (env, admin, client) = setup();
        let addr = Address::generate(&env);
        // 65 alphanumeric chars — exceeds max length of 64
        let long_name = String::from_str(&env, &"a".repeat(65));
        let result = client.try_register_route(&admin, &long_name, &addr, &None);
        assert_eq!(result, Err(Ok(RouterError::InvalidRouteName)));
    }

    #[test]
    fn test_register_name_at_max_length_succeeds() {
        let (env, admin, client) = setup();
        let addr = Address::generate(&env);
        // Exactly 64 chars — must succeed
        let name = String::from_str(&env, &"a".repeat(64));
        assert!(client.try_register_route(&admin, &name, &addr, &None).is_ok());
    }

    #[test]
    fn test_register_name_with_special_chars_fails() {
        let (env, admin, client) = setup();
        let addr = Address::generate(&env);
        // Underscore is not allowed
        let result = client.try_register_route(&admin, &String::from_str(&env, "oracle_v1"), &addr, &None);
        assert_eq!(result, Err(Ok(RouterError::InvalidRouteName)));
    }

    #[test]
    fn test_register_name_with_slash_and_hyphen_succeeds() {
        let (env, admin, client) = setup();
        let addr = Address::generate(&env);
        // Slash and hyphen are allowed
        let name = String::from_str(&env, "oracle/get-price");
        assert!(client.try_register_route(&admin, &name, &addr, &None).is_ok());
    }

    #[test]
    fn test_get_all_routes_updates_after_remove() {
        let (env, admin, client) = setup();
        let oracle = String::from_str(&env, "oracle");
        let vault = String::from_str(&env, "vault");
        let addr1 = Address::generate(&env);
        let addr2 = Address::generate(&env);

        client.register_route(&admin, &oracle, &addr1, &None);
        client.register_route(&admin, &vault, &addr2, &None);
        assert_eq!(client.get_all_routes().len(), 2);

        client.remove_route(&admin, &oracle);
        let routes = client.get_all_routes();
        assert_eq!(routes.len(), 1);
        assert!(!routes.contains(&oracle));
        assert!(routes.contains(&vault));
    }

    #[test]
    fn test_get_all_routes_re_register_after_remove() {
        let (env, admin, client) = setup();
        let oracle = String::from_str(&env, "oracle");
        let addr1 = Address::generate(&env);
        let addr2 = Address::generate(&env);

        client.register_route(&admin, &oracle, &addr1, &None);
        assert_eq!(client.get_all_routes().len(), 1);

        client.remove_route(&admin, &oracle);
        assert_eq!(client.get_all_routes().len(), 0);

        client.register_route(&admin, &oracle, &addr2, &None);
        let routes = client.get_all_routes();
        assert_eq!(routes.len(), 1);
        assert!(routes.contains(&oracle));
    }

    #[test]
    fn test_pause_all_blocks_new_resolutions() {
        let (env, admin, client) = setup();
        let name = String::from_str(&env, "oracle");
        let addr = Address::generate(&env);
        client.register_route(&admin, &name, &addr, &None);

        // Verify resolve works before pause
        assert_eq!(client.resolve(&name), addr);

        // Pause the router
        client.set_paused(&admin, &true);

        // Verify resolve fails after pause
        let result = client.try_resolve(&name);
        assert_eq!(result, Err(Ok(RouterError::RouterPaused)));
    }

    #[test]
    fn test_pause_all_checked_before_route_lookup() {
        let (env, admin, client) = setup();
        let name = String::from_str(&env, "oracle");
        let addr = Address::generate(&env);
        client.register_route(&admin, &name, &addr, &None);

        // Pause the router
        client.set_paused(&admin, &true);

        // Even with a valid route, resolve should fail with RouterPaused, not RouteNotFound
        let result = client.try_resolve(&name);
        assert_eq!(result, Err(Ok(RouterError::RouterPaused)));
    }

    #[test]
    fn test_add_alias_resolves_to_original() {
        let (env, admin, client) = setup();
        let name = String::from_str(&env, "oracle");
        let alias = String::from_str(&env, "oracle-v1");
        let addr = Address::generate(&env);
        client.register_route(&admin, &name, &addr, &None);
        client.add_alias(&admin, &name, &alias);
        assert_eq!(client.resolve(&alias), addr);
    }

    #[test]
    fn test_remove_alias() {
        let (env, admin, client) = setup();
        let name = String::from_str(&env, "oracle");
        let alias = String::from_str(&env, "oracle-v1");
        let addr = Address::generate(&env);
        client.register_route(&admin, &name, &addr, &None);
        client.add_alias(&admin, &name, &alias);
        client.remove_alias(&admin, &alias);
        let result = client.try_resolve(&alias);
        assert_eq!(result, Err(Ok(RouterError::RouteNotFound)));
    }

    #[test]
    fn test_alias_for_nonexistent_route_fails() {
        let (env, admin, client) = setup();
        let name = String::from_str(&env, "oracle");
        let alias = String::from_str(&env, "oracle-v1");
        let result = client.try_add_alias(&admin, &name, &alias);
        assert_eq!(result, Err(Ok(RouterError::RouteNotFound)));
    }

    #[test]
    fn test_alias_name_cannot_be_existing_route() {
        let (env, admin, client) = setup();
        let name1 = String::from_str(&env, "oracle");
        let name2 = String::from_str(&env, "vault");
        let addr1 = Address::generate(&env);
        let addr2 = Address::generate(&env);
        client.register_route(&admin, &name1, &addr1, &None);
        client.register_route(&admin, &name2, &addr2, &None);
        let result = client.try_add_alias(&admin, &name1, &name2);
        assert_eq!(result, Err(Ok(RouterError::RouteAlreadyExists)));
    }

    #[test]
    fn test_resolve_dangling_alias_returns_route_not_found() {
        let (env, admin, client) = setup();
        let name = String::from_str(&env, "oracle");
        let alias = String::from_str(&env, "oracle-v1");
        let addr = Address::generate(&env);

        client.register_route(&admin, &name, &addr, &None);
        client.add_alias(&admin, &name, &alias);

        // Remove the underlying route
        client.remove_route(&admin, &name);

        // Alias key still exists, but target route is gone
        assert_eq!(
            client.try_resolve(&alias),
            Err(Ok(RouterError::RouteNotFound))
        );
    }

    #[test]
    fn test_add_alias_to_removed_route_fails() {
        let (env, admin, client) = setup();
        let name = String::from_str(&env, "oracle");
        let alias = String::from_str(&env, "oracle-alias");
        let addr = Address::generate(&env);

        client.register_route(&admin, &name, &addr, &None);
        client.remove_route(&admin, &name);

        // Should fail — target route no longer exists
        assert_eq!(
            client.try_add_alias(&admin, &name, &alias),
            Err(Ok(RouterError::RouteNotFound))
        );
    }

    #[test]
    fn test_register_route_with_metadata() {
        let (env, admin, client) = setup();
        let name = String::from_str(&env, "oracle");
        let addr = Address::generate(&env);
        let description = String::from_str(&env, "Oracle price feed");
        let tags = vec![
            &env,
            String::from_str(&env, "defi"),
            String::from_str(&env, "oracle"),
        ];
        let owner = admin.clone();

        let metadata = Some(RouteMetadata {
            description: description.clone(),
            tags: tags.clone(),
            owner: owner.clone(),
        });

        client.register_route(&admin, &name, &addr, &metadata);

        let retrieved_metadata = client.get_metadata(&name);
        assert_eq!(retrieved_metadata, metadata);
    }

    #[test]
    fn test_update_metadata() {
        let (env, admin, client) = setup();
        let name = String::from_str(&env, "oracle");
        let addr = Address::generate(&env);

        client.register_route(&admin, &name, &addr, &None);

        let description = String::from_str(&env, "Updated oracle");
        let tags = vec![&env, String::from_str(&env, "v2")];
        let metadata = Some(RouteMetadata {
            description,
            tags,
            owner: Address::generate(&env),
        });

        let events_before = env.events().all().len();
        client.update_metadata(&admin, &name, &metadata);
        let events_after = env.events().all().len();

        assert_eq!(events_after, events_before + 1);

        let retrieved = client.get_metadata(&name);
        assert_eq!(retrieved, metadata);
    }

    #[test]
    fn test_metadata_updated_event_includes_metadata() {
        let (env, admin, client) = setup();
        let name = String::from_str(&env, "oracle");
        let addr = Address::generate(&env);
        let description = String::from_str(&env, "Test metadata");
        let tags = vec![&env, String::from_str(&env, "test")];
        let metadata = Some(RouteMetadata {
            description,
            tags,
            owner: Address::generate(&env),
        });

        client.register_route(&admin, &name, &addr, &None);
        client.update_metadata(&admin, &name, &metadata);

        let event = env.events().all().last().unwrap().clone();
        assert_eq!(event.0, client.address);
        assert_eq!(
            event.1,
            vec![&env, Symbol::new(&env, "metadata_updated").into_val(&env)]
        );

        let (emitted_name, has_metadata): (String, bool) = event.2.into_val(&env);
        assert_eq!(emitted_name, name);
        assert!(has_metadata);
    }

    #[test]
    fn test_metadata_updated_event_when_cleared() {
        let (env, admin, client) = setup();
        let name = String::from_str(&env, "oracle");
        let addr = Address::generate(&env);

        let description = String::from_str(&env, "Initial metadata");
        let tags = vec![&env, String::from_str(&env, "test")];
        let metadata = Some(RouteMetadata {
            description,
            tags,
            owner: Address::generate(&env),
        });

        client.register_route(&admin, &name, &addr, &metadata);

        client.update_metadata(&admin, &name, &None);

        let events = env.events().all();
        let meta_event = events
            .iter()
            .find(|e| {
                e.1.get(0)
                    .map(|v| {
                        let s: Symbol = v.into_val(&env);
                        s == Symbol::new(&env, "metadata_updated")
                    })
                    .unwrap_or(false)
            })
            .unwrap();

        let (_emitted_name, has_metadata): (String, bool) = meta_event.2.into_val(&env);
        assert!(!has_metadata);
    }

    // ── RouteMetadata validation tests (issues #180 & #191) ──────────────────

    #[test]
    fn test_set_route_paused_updates_updated_by() {
        let (env, admin, client) = setup();
        let name = String::from_str(&env, "oracle");
        let addr = Address::generate(&env);

        // Register a route with admin A
        client.register_route(&admin, &name, &addr, &None);

        // Verify initial updated_by is admin
        let entry = client.get_route(&name).unwrap();
        assert_eq!(entry.updated_by, admin);

        // Transfer admin to B
        let new_admin = Address::generate(&env);
        client.transfer_admin(&admin, &new_admin);

        // Pause with B
        client.set_route_paused(&new_admin, &name, &true);

        // Verify updated_by is now B
        let entry = client.get_route(&name).unwrap();
        assert_eq!(entry.updated_by, new_admin);
    }

    #[test]
    fn test_resolve_alias_to_paused_route_fails() {
        let (env, admin, client) = setup();
        let name = String::from_str(&env, "oracle");
        let alias = String::from_str(&env, "oracle-v1");
        let addr = Address::generate(&env);
        client.register_route(&admin, &name, &addr, &None);
        client.add_alias(&admin, &name, &alias);
        client.set_route_paused(&admin, &name, &true);
        // Resolving alias should fail with RoutePaused
        assert_eq!(
            client.try_resolve(&alias),
            Err(Ok(RouterError::RoutePaused))
        );
    }

    // ── RouteMetadata validation tests (issues #180 & #191) ──────────────────

    #[test]
    fn test_update_metadata_description_too_long_returns_invalid_metadata() {
        let (env, admin, client) = setup();
        let name = String::from_str(&env, "oracle");
        let addr = Address::generate(&env);
        client.register_route(&admin, &name, &addr, &None);

        // 257-char description — must fail
        let long_desc = String::from_str(&env, &"a".repeat(257));
        let metadata = Some(RouteMetadata {
            description: long_desc,
            tags: Vec::new(&env),
            owner: Address::generate(&env),
        });
        assert_eq!(
            client.try_update_metadata(&admin, &name, &metadata),
            Err(Ok(RouterError::InvalidMetadata))
        );
    }

    #[test]
    fn test_update_metadata_too_many_tags_returns_invalid_metadata() {
        let (env, admin, client) = setup();
        let name = String::from_str(&env, "oracle");
        let addr = Address::generate(&env);
        client.register_route(&admin, &name, &addr, &None);

        // 6 tags — must fail
        let mut tags = Vec::new(&env);
        for i in 0..6u32 {
            tags.push_back(String::from_str(&env, &i.to_string()));
        }
        let metadata = Some(RouteMetadata {
            description: String::from_str(&env, "valid"),
            tags,
            owner: Address::generate(&env),
        });
        assert_eq!(
            client.try_update_metadata(&admin, &name, &metadata),
            Err(Ok(RouterError::InvalidMetadata))
        );
    }

    #[test]
    fn test_resolve_alias_to_paused_route_emits_canonical_name() {
        let (env, admin, client) = setup();
        let name = String::from_str(&env, "oracle");
        let alias = String::from_str(&env, "oracle-v1");
        let addr = Address::generate(&env);

        client.register_route(&admin, &name, &addr, &None);
        client.add_alias(&admin, &name, &alias);
        client.set_route_paused(&admin, &name, &true);

        // Attempt to resolve the paused alias
        let _ = client.try_resolve(&alias);

        // Verify the route_resolve_paused event was emitted with canonical name
        let event = env.events().all().last().unwrap().clone();
        assert_eq!(event.0, client.address);
        assert_eq!(
            event.1,
            vec![
                &env,
                Symbol::new(&env, "route_resolve_paused").into_val(&env)
            ]
        );
        let (emitted_name,): (String,) = event.2.into_val(&env);
        assert_eq!(emitted_name, name); // Should be canonical name, not alias
        assert_ne!(emitted_name, alias); // Explicitly verify it's not the alias
    }

    #[test]
    fn test_update_metadata_valid_succeeds() {
        let (env, admin, client) = setup();
        let name = String::from_str(&env, "oracle");
        let addr = Address::generate(&env);
        client.register_route(&admin, &name, &addr, &None);

        let mut tags = Vec::new(&env);
        tags.push_back(String::from_str(&env, "defi"));
        let metadata = Some(RouteMetadata {
            description: String::from_str(&env, "valid description"),
            tags,
            owner: Address::generate(&env),
        });
        assert!(client.try_update_metadata(&admin, &name, &metadata).is_ok());
        assert_eq!(client.get_metadata(&name), metadata);
    }

    #[test]
    fn test_update_metadata_clears_when_none() {
        let (env, admin, client) = setup();
        let name = String::from_str(&env, "oracle");
        let addr = Address::generate(&env);

        let metadata = Some(RouteMetadata {
            description: String::from_str(&env, "initial"),
            tags: Vec::new(&env),
            owner: Address::generate(&env),
        });
        client.register_route(&admin, &name, &addr, &metadata);
        assert!(client.get_metadata(&name).is_some());

        client.update_metadata(&admin, &name, &None);
        assert_eq!(client.get_metadata(&name), None);
    }

    #[test]
    fn test_update_metadata_description_at_limit() {
        let (env, admin, client) = setup();
        let name = String::from_str(&env, "oracle");
        let addr = Address::generate(&env);
        client.register_route(&admin, &name, &addr, &None);

        // Exactly 256 chars — must succeed
        let desc = String::from_str(&env, &"a".repeat(256));
        let metadata = Some(RouteMetadata {
            description: desc,
            tags: Vec::new(&env),
            owner: Address::generate(&env),
        });
        assert!(client.try_update_metadata(&admin, &name, &metadata).is_ok());
    }

    #[test]
    fn test_update_metadata_description_over_limit() {
        let (env, admin, client) = setup();
        let name = String::from_str(&env, "oracle");
        let addr = Address::generate(&env);
        client.register_route(&admin, &name, &addr, &None);

        // 257 chars — must fail
        let desc = String::from_str(&env, &"a".repeat(257));
        let metadata = Some(RouteMetadata {
            description: desc,
            tags: Vec::new(&env),
            owner: Address::generate(&env),
        });
        assert_eq!(
            client.try_update_metadata(&admin, &name, &metadata),
            Err(Ok(RouterError::InvalidMetadata))
        );
    }

    #[test]
    fn test_update_metadata_tags_at_limit() {
        let (env, admin, client) = setup();
        let name = String::from_str(&env, "oracle");
        let addr = Address::generate(&env);
        client.register_route(&admin, &name, &addr, &None);

        // Exactly 5 tags — must succeed
        let mut tags = Vec::new(&env);
        for i in 0..5u32 {
            tags.push_back(String::from_str(&env, &i.to_string()));
        }
        let metadata = Some(RouteMetadata {
            description: String::from_str(&env, "valid"),
            tags,
            owner: Address::generate(&env),
        });
        assert!(client.try_update_metadata(&admin, &name, &metadata).is_ok());
    }

    #[test]
    fn test_update_metadata_tags_over_limit() {
        let (env, admin, client) = setup();
        let name = String::from_str(&env, "oracle");
        let addr = Address::generate(&env);
        client.register_route(&admin, &name, &addr, &None);

        // 6 tags — must fail
        let mut tags = Vec::new(&env);
        for i in 0..6u32 {
            tags.push_back(String::from_str(&env, &i.to_string()));
        }
        let metadata = Some(RouteMetadata {
            description: String::from_str(&env, "valid"),
            tags,
            owner: Address::generate(&env),
        });
        assert_eq!(
            client.try_update_metadata(&admin, &name, &metadata),
            Err(Ok(RouterError::InvalidMetadata))
        );
    }

    #[test]
    fn test_get_metadata_nonexistent_route_returns_none() {
        let (env, _admin, client) = setup();
        let name = String::from_str(&env, "nonexistent");
        assert_eq!(client.get_metadata(&name), None);
    }

    #[test]
    fn test_remove_route_cleans_up_dangling_aliases() {
        let (env, admin, client) = setup();
        let oracle = String::from_str(&env, "oracle");
        let oracle_v1 = String::from_str(&env, "oracle-v1"); // Rust variable keeps old name for readability
        let addr = Address::generate(&env);

        // Register route and create alias
        client.register_route(&admin, &oracle, &addr, &None);
        client.add_alias(&admin, &oracle, &oracle_v1);

        // Verify alias works initially
        assert_eq!(client.resolve(&oracle_v1), addr);

        // Remove the original route
        client.remove_route(&admin, &oracle);

        // Alias should now return RouteNotFound (not dangling)
        assert_eq!(
            client.try_resolve(&oracle_v1),
            Err(Ok(RouterError::RouteNotFound))
        );

        // Original route should also return RouteNotFound
        assert_eq!(
            client.try_resolve(&oracle),
            Err(Ok(RouterError::RouteNotFound))
        );
    }

    #[test]
    fn test_total_routed_increments_on_alias_resolution() {
        let (env, admin, client) = setup();
        let name = String::from_str(&env, "oracle");
        let alias = String::from_str(&env, "oracle-v1");
        let addr = Address::generate(&env);

        client.register_route(&admin, &name, &addr, &None);
        client.add_alias(&admin, &name, &alias);

        assert_eq!(client.total_routed(), 0);
        client.resolve(&alias);
        assert_eq!(client.total_routed(), 1);  // alias resolution increments counter
        client.resolve(&name);
        assert_eq!(client.total_routed(), 2);
    }

    #[test]
    fn test_update_metadata_nonexistent_route_fails() {
        let (env, admin, client) = setup();
        let name = String::from_str(&env, "ghost");
        let result = client.try_update_metadata(&admin, &name, &None);
        assert_eq!(result, Err(Ok(RouterError::RouteNotFound)));
    }

    #[test]
    fn test_update_metadata_unauthorized_fails() {
        let (env, admin, client) = setup();
        let name = String::from_str(&env, "oracle");
        let addr = Address::generate(&env);
        client.register_route(&admin, &name, &addr, &None);
        let attacker = Address::generate(&env);
        let result = client.try_update_metadata(&attacker, &name, &None);
        assert_eq!(result, Err(Ok(RouterError::Unauthorized)));
    }

    #[test]
    fn test_get_alias_target_returns_canonical_name() {
        let (env, admin, client) = setup();
        let name = String::from_str(&env, "oracle");
        let alias = String::from_str(&env, "oracle-v1");
        let addr = Address::generate(&env);
        client.register_route(&admin, &name, &addr, &None);
        client.add_alias(&admin, &name, &alias);
        assert_eq!(client.get_alias_target(&alias), Some(name));
    }

    #[test]
    fn test_get_alias_target_returns_none_for_non_alias() {
        let (env, _admin, client) = setup();
        let name = String::from_str(&env, "not_an_alias");
        assert_eq!(client.get_alias_target(&name), None);
    }

    // ── Issue #453: alias edge cases ─────────────────────────────────────────

    #[test]
    fn test_alias_chain_resolves_to_original_address() {
        // alias_b → oracle (alias pointing to another alias is not supported;
        // add_alias only accepts existing *routes*, not aliases, as the target).
        // This test verifies that an alias of an alias is rejected with RouteNotFound
        // because the intermediate alias is not a registered route.
        let (env, admin, client) = setup();
        let oracle = String::from_str(&env, "oracle");
        let alias_a = String::from_str(&env, "oracle_a");
        let alias_b = String::from_str(&env, "oracle_b");
        let addr = Address::generate(&env);

        client.register_route(&admin, &oracle, &addr, &None);
        client.add_alias(&admin, &oracle, &alias_a);

        // alias_a is not a route, so aliasing it should fail
        let result = client.try_add_alias(&admin, &alias_a, &alias_b);
        assert_eq!(result, Err(Ok(RouterError::RouteNotFound)));
    }

    #[test]
    fn test_dangling_alias_after_parent_route_removed() {
        // After remove_route, the alias is cleaned up and resolving it returns RouteNotFound.
        let (env, admin, client) = setup();
        let oracle = String::from_str(&env, "oracle");
        let alias = String::from_str(&env, "oracle_v1");
        let addr = Address::generate(&env);

        client.register_route(&admin, &oracle, &addr, &None);
        client.add_alias(&admin, &oracle, &alias);
        assert_eq!(client.resolve(&alias), addr);

        client.remove_route(&admin, &oracle);

        assert_eq!(
            client.try_resolve(&alias),
            Err(Ok(RouterError::RouteNotFound))
        );
        // The alias target lookup should also return None after cleanup
        assert_eq!(client.get_alias_target(&alias), None);
    }

    #[test]
    fn test_duplicate_alias_name_fails() {
        // Creating an alias with the same name as an existing alias returns RouteAlreadyExists.
        let (env, admin, client) = setup();
        let oracle = String::from_str(&env, "oracle");
        let alias = String::from_str(&env, "oracle_v1");
        let addr = Address::generate(&env);

        client.register_route(&admin, &oracle, &addr, &None);
        client.add_alias(&admin, &oracle, &alias);

        // Second add_alias with the same alias name must fail
        let result = client.try_add_alias(&admin, &oracle, &alias);
        assert_eq!(result, Err(Ok(RouterError::RouteAlreadyExists)));
    }

    // ── Route scoring / path selection tests (#330) ───────────────────────────

    #[test]
    fn test_set_and_get_route_score() {
        let (env, admin, client) = setup();
        let name = String::from_str(&env, "oracle");
        let addr = Address::generate(&env);
        client.register_route(&admin, &name, &addr, &None);

        let score = RouteScore { liquidity_score: 80, fee_bps: 30, reliability_score: 90 };
        client.set_route_score(&admin, &name, &score);

        let retrieved = client.get_route_score(&name).unwrap();
        assert_eq!(retrieved.liquidity_score, 80);
        assert_eq!(retrieved.fee_bps, 30);
        assert_eq!(retrieved.reliability_score, 90);
    }

    #[test]
    fn test_set_route_score_nonexistent_fails() {
        let (env, admin, client) = setup();
        let name = String::from_str(&env, "ghost");
        let score = RouteScore { liquidity_score: 50, fee_bps: 10, reliability_score: 50 };
        let result = client.try_set_route_score(&admin, &name, &score);
        assert_eq!(result, Err(Ok(RouterError::RouteNotFound)));
    }

    #[test]
    fn test_get_best_route_selects_highest_score() {
        let (env, admin, client) = setup();
        let r1 = String::from_str(&env, "route-a");
        let r2 = String::from_str(&env, "route-b");
        let r3 = String::from_str(&env, "route-c");
        let addr = Address::generate(&env);

        client.register_route(&admin, &r1, &addr, &None);
        client.register_route(&admin, &r2, &addr, &None);
        client.register_route(&admin, &r3, &addr, &None);

        // route_a: 50 + 70 - 30/10 = 117
        client.set_route_score(&admin, &r1, &RouteScore { liquidity_score: 50, fee_bps: 30, reliability_score: 70 });
        // route_b: 90 + 95 - 10/10 = 184  ← best
        client.set_route_score(&admin, &r2, &RouteScore { liquidity_score: 90, fee_bps: 10, reliability_score: 95 });
        // route_c: 60 + 60 - 50/10 = 115
        client.set_route_score(&admin, &r3, &RouteScore { liquidity_score: 60, fee_bps: 50, reliability_score: 60 });

        let candidates = vec![&env, r1, r2.clone(), r3];
        let best = client.get_best_route(&candidates, &0, &None);
        assert_eq!(best, Some(r2));
    }

    #[test]
    fn test_get_best_route_skips_paused() {
        let (env, admin, client) = setup();
        let r1 = String::from_str(&env, "route-a");
        let r2 = String::from_str(&env, "route-b");
        let addr = Address::generate(&env);

        client.register_route(&admin, &r1, &addr, &None);
        client.register_route(&admin, &r2, &addr, &None);

        // r1 has higher score but is paused
        client.set_route_score(&admin, &r1, &RouteScore { liquidity_score: 100, fee_bps: 0, reliability_score: 100 });
        client.set_route_score(&admin, &r2, &RouteScore { liquidity_score: 50, fee_bps: 10, reliability_score: 50 });
        client.set_route_paused(&admin, &r1, &true);

        let candidates = vec![&env, r1, r2.clone()];
        let best = client.get_best_route(&candidates, &0, &None);
        assert_eq!(best, Some(r2));
    }

    #[test]
    fn test_get_best_route_returns_none_when_all_unscored() {
        let (env, admin, client) = setup();
        let r1 = String::from_str(&env, "route-a");
        let addr = Address::generate(&env);
        client.register_route(&admin, &r1, &addr, &None);
        // No score set
        let candidates = vec![&env, r1];
        let best = client.get_best_route(&candidates, &0, &None);
        assert_eq!(best, None);
    }

    #[test]
    fn test_get_best_route_fails_when_router_paused() {
        let (env, admin, client) = setup();
        client.set_paused(&admin, &true);
        let candidates = Vec::new(&env);
        let result = client.try_get_best_route(&candidates, &0, &None);
        assert_eq!(result, Err(Ok(RouterError::RouterPaused)));
    }

    #[test]
    fn test_get_best_route_fallback_when_below_min_score() {
        let (env, admin, client) = setup();
        let r1 = String::from_str(&env, "route-a");
        let fallback = String::from_str(&env, "fallback-route");
        let addr = Address::generate(&env);
        client.register_route(&admin, &r1, &addr, &None);
        // route_a: 50 + 50 - 10/10 = 99
        client.set_route_score(&admin, &r1, &RouteScore { liquidity_score: 50, fee_bps: 10, reliability_score: 50 });

        let candidates = vec![&env, r1];
        // min_score = 200 — route_a (99) doesn't qualify → fallback returned
        let best = client.get_best_route(&candidates, &200, &Some(fallback.clone()));
        assert_eq!(best, Some(fallback));
    }

    #[test]
    fn test_get_best_route_no_fallback_returns_none_when_below_min_score() {
        let (env, admin, client) = setup();
        let r1 = String::from_str(&env, "route-a");
        let addr = Address::generate(&env);
        client.register_route(&admin, &r1, &addr, &None);
        client.set_route_score(&admin, &r1, &RouteScore { liquidity_score: 10, fee_bps: 0, reliability_score: 10 });

        let candidates = vec![&env, r1];
        // min_score = 1000 — no route qualifies, no fallback
        let best = client.get_best_route(&candidates, &1000, &None);
        assert_eq!(best, None);
    }

    // ── Issue #506: get_all_routes after remove and re-register ──────────────

    #[test]
    fn test_get_all_routes_count_decrements_after_remove() {
        let (env, admin, client) = setup();
        let oracle = String::from_str(&env, "oracle");
        let vault = String::from_str(&env, "vault");
        let swap = String::from_str(&env, "swap");
        let addr = Address::generate(&env);

        // Register 3 routes
        client.register_route(&admin, &oracle, &addr, &None);
        client.register_route(&admin, &vault, &addr, &None);
        client.register_route(&admin, &swap, &addr, &None);
        assert_eq!(client.get_all_routes().len(), 3);

        // Remove one route
        client.remove_route(&admin, &vault);
        
        // Count should decrement by one
        let routes = client.get_all_routes();
        assert_eq!(routes.len(), 2);
        assert!(routes.contains(&oracle));
        assert!(!routes.contains(&vault));
        assert!(routes.contains(&swap));
    }

    #[test]
    fn test_get_all_routes_includes_re_registered_route() {
        let (env, admin, client) = setup();
        let oracle = String::from_str(&env, "oracle");
        let addr1 = Address::generate(&env);
        let addr2 = Address::generate(&env);

        // Register, remove, then re-register
        client.register_route(&admin, &oracle, &addr1, &None);
        assert_eq!(client.get_all_routes().len(), 1);

        client.remove_route(&admin, &oracle);
        assert_eq!(client.get_all_routes().len(), 0);

        client.register_route(&admin, &oracle, &addr2, &None);
        
        // Route should be back in the list
        let routes = client.get_all_routes();
        assert_eq!(routes.len(), 1);
        assert!(routes.contains(&oracle));
    }

    #[test]
    fn test_get_all_routes_no_duplicates_after_multiple_operations() {
        let (env, admin, client) = setup();
        let oracle = String::from_str(&env, "oracle");
        let vault = String::from_str(&env, "vault");
        let swap = String::from_str(&env, "swap");
        let addr = Address::generate(&env);

        // Register multiple routes
        client.register_route(&admin, &oracle, &addr, &None);
        client.register_route(&admin, &vault, &addr, &None);
        client.register_route(&admin, &swap, &addr, &None);

        // Remove and re-register some routes
        client.remove_route(&admin, &vault);
        client.register_route(&admin, &vault, &addr, &None);
        client.remove_route(&admin, &oracle);
        client.register_route(&admin, &oracle, &addr, &None);

        // Verify no duplicates
        let routes = client.get_all_routes();
        assert_eq!(routes.len(), 3);
        
        // Count occurrences of each route name
        let mut oracle_count = 0;
        let mut vault_count = 0;
        let mut swap_count = 0;
        for route in routes.iter() {
            if route == oracle {
                oracle_count += 1;
            } else if route == vault {
                vault_count += 1;
            } else if route == swap {
                swap_count += 1;
            }
        }
        
        assert_eq!(oracle_count, 1, "oracle should appear exactly once");
        assert_eq!(vault_count, 1, "vault should appear exactly once");
        assert_eq!(swap_count, 1, "swap should appear exactly once");
    }

    // ── Issue #511: Route validation tests ───────────────────────────────────

    #[test]
    fn test_validate_route_name_rejects_empty_string() {
        let (env, admin, client) = setup();
        let empty_name = String::from_str(&env, "");
        let addr = Address::generate(&env);
        let result = client.try_register_route(&admin, &empty_name, &addr, &None);
        assert_eq!(result, Err(Ok(RouterError::InvalidRouteName)));
    }

    #[test]
    fn test_validate_route_name_rejects_whitespace_only() {
        let (env, admin, client) = setup();
        let whitespace_name = String::from_str(&env, "   ");
        let addr = Address::generate(&env);
        let result = client.try_register_route(&admin, &whitespace_name, &addr, &None);
        assert_eq!(result, Err(Ok(RouterError::InvalidRouteName)));
    }

    #[test]
    fn test_validate_route_name_prevents_duplicate_route() {
        let (env, admin, client) = setup();
        let name = String::from_str(&env, "oracle");
        let addr1 = Address::generate(&env);
        let addr2 = Address::generate(&env);
        
        client.register_route(&admin, &name, &addr1, &None);
        let result = client.try_register_route(&admin, &name, &addr2, &None);
        assert_eq!(result, Err(Ok(RouterError::RouteAlreadyExists)));
    }

    #[test]
    fn test_validate_route_name_prevents_alias_as_route() {
        let (env, admin, client) = setup();
        let route_name = String::from_str(&env, "oracle");
        let alias_name = String::from_str(&env, "oracle-v1");
        let addr1 = Address::generate(&env);
        let addr2 = Address::generate(&env);
        
        // Register route and create alias
        client.register_route(&admin, &route_name, &addr1, &None);
        client.add_alias(&admin, &route_name, &alias_name);
        
        // Try to register a route with the same name as the alias
        let result = client.try_register_route(&admin, &alias_name, &addr2, &None);
        assert_eq!(result, Err(Ok(RouterError::RouteAlreadyExists)));
    }

    #[test]
    fn test_validate_route_name_prevents_route_as_alias() {
        let (env, admin, client) = setup();
        let route1 = String::from_str(&env, "oracle");
        let route2 = String::from_str(&env, "vault");
        let addr1 = Address::generate(&env);
        let addr2 = Address::generate(&env);
        
        // Register two routes
        client.register_route(&admin, &route1, &addr1, &None);
        client.register_route(&admin, &route2, &addr2, &None);
        
        // Try to create an alias with the same name as an existing route
        let result = client.try_add_alias(&admin, &route1, &route2);
        assert_eq!(result, Err(Ok(RouterError::RouteAlreadyExists)));
    }

    #[test]
    fn test_validate_route_name_alias_empty_string_fails() {
        let (env, admin, client) = setup();
        let route_name = String::from_str(&env, "oracle");
        let empty_alias = String::from_str(&env, "");
        let addr = Address::generate(&env);
        
        client.register_route(&admin, &route_name, &addr, &None);
        let result = client.try_add_alias(&admin, &route_name, &empty_alias);
        assert_eq!(result, Err(Ok(RouterError::InvalidRouteName)));
    }

    #[test]
    fn test_validate_route_name_alias_whitespace_fails() {
        let (env, admin, client) = setup();
        let route_name = String::from_str(&env, "oracle");
        let whitespace_alias = String::from_str(&env, "\t\n ");
        let addr = Address::generate(&env);
        
        client.register_route(&admin, &route_name, &addr, &None);
        let result = client.try_add_alias(&admin, &route_name, &whitespace_alias);
        assert_eq!(result, Err(Ok(RouterError::InvalidRouteName)));
    }

    #[test]
    fn test_route_count() {
        let (env, admin, client) = setup();
        assert_eq!(client.route_count(), 0);

        let name1 = String::from_str(&env, "oracle");
        let addr1 = Address::generate(&env);
        client.register_route(&admin, &name1, &addr1, &None);
        assert_eq!(client.route_count(), 1);

        let name2 = String::from_str(&env, "vault");
        let addr2 = Address::generate(&env);
        client.register_route(&admin, &name2, &addr2, &None);
        assert_eq!(client.route_count(), 2);

        client.remove_route(&admin, &name1);
        assert_eq!(client.route_count(), 1);
    }

    #[test]
    fn test_route_count_reregister_does_not_double_count() {
        let (env, admin, client) = setup();
        let name = String::from_str(&env, "oracle");
        let addr1 = Address::generate(&env);
        let addr2 = Address::generate(&env);

        client.register_route(&admin, &name, &addr1, &None);
        assert_eq!(client.route_count(), 1);

        client.remove_route(&admin, &name);
        assert_eq!(client.route_count(), 0);

        client.register_route(&admin, &name, &addr2, &None);
        assert_eq!(client.route_count(), 1);
    }

    // ── batch_resolve tests ───────────────────────────────────────────────────

    #[test]
    fn test_batch_resolve_all_succeed() {
        let (env, admin, client) = setup();
        let oracle = String::from_str(&env, "oracle");
        let vault = String::from_str(&env, "vault");
        let addr1 = Address::generate(&env);
        let addr2 = Address::generate(&env);
        client.register_route(&admin, &oracle, &addr1, &None);
        client.register_route(&admin, &vault, &addr2, &None);

        let names = vec![&env, oracle, vault];
        let results = client.batch_resolve(&names);

        assert_eq!(results.len(), 2);
        assert_eq!(results.get(0).unwrap(), BatchResolveResult::Ok(addr1));
        assert_eq!(results.get(1).unwrap(), BatchResolveResult::Ok(addr2));
    }

    #[test]
    fn test_batch_resolve_empty_input() {
        let (env, _admin, client) = setup();
        let names: Vec<String> = Vec::new(&env);
        let results = client.batch_resolve(&names);
        assert_eq!(results.len(), 0);
    }

    #[test]
    fn test_batch_resolve_partial_failure() {
        let (env, admin, client) = setup();
        let oracle = String::from_str(&env, "oracle");
        let missing = String::from_str(&env, "missing");
        let addr = Address::generate(&env);
        client.register_route(&admin, &oracle, &addr, &None);

        let names = vec![&env, oracle, missing];
        let results = client.batch_resolve(&names);

        assert_eq!(results.len(), 2);
        assert_eq!(results.get(0).unwrap(), BatchResolveResult::Ok(addr));
        assert_eq!(
            results.get(1).unwrap(),
            BatchResolveResult::Err(ResolveError::RouteNotFound)
        );
    }

    #[test]
    fn test_batch_resolve_router_paused_all_fail() {
        let (env, admin, client) = setup();
        let oracle = String::from_str(&env, "oracle");
        let addr = Address::generate(&env);
        client.register_route(&admin, &oracle, &addr, &None);
        client.set_paused(&admin, &true);

        let names = vec![&env, oracle];
        let results = client.batch_resolve(&names);

        assert_eq!(results.len(), 1);
        assert_eq!(
            results.get(0).unwrap(),
            BatchResolveResult::Err(ResolveError::RouterPaused)
        );
    }

    #[test]
    fn test_batch_resolve_paused_route_returns_err() {
        let (env, admin, client) = setup();
        let oracle = String::from_str(&env, "oracle");
        let vault = String::from_str(&env, "vault");
        let addr1 = Address::generate(&env);
        let addr2 = Address::generate(&env);
        client.register_route(&admin, &oracle, &addr1, &None);
        client.register_route(&admin, &vault, &addr2, &None);
        client.set_route_paused(&admin, &oracle, &true);

        let names = vec![&env, oracle, vault];
        let results = client.batch_resolve(&names);

        assert_eq!(results.len(), 2);
        assert_eq!(
            results.get(0).unwrap(),
            BatchResolveResult::Err(ResolveError::RoutePaused)
        );
        assert_eq!(results.get(1).unwrap(), BatchResolveResult::Ok(addr2));
    }

    #[test]
    fn test_batch_resolve_preserves_order() {
        let (env, admin, client) = setup();
        let r1 = String::from_str(&env, "route1");
        let r2 = String::from_str(&env, "route2");
        let r3 = String::from_str(&env, "route3");
        let addr1 = Address::generate(&env);
        let addr2 = Address::generate(&env);
        let addr3 = Address::generate(&env);
        client.register_route(&admin, &r1, &addr1, &None);
        client.register_route(&admin, &r2, &addr2, &None);
        client.register_route(&admin, &r3, &addr3, &None);

        // Intentionally reverse order to verify output order matches input
        let names = vec![&env, r3, r1, r2];
        let results = client.batch_resolve(&names);

        assert_eq!(results.len(), 3);
        assert_eq!(results.get(0).unwrap(), BatchResolveResult::Ok(addr3));
        assert_eq!(results.get(1).unwrap(), BatchResolveResult::Ok(addr1));
        assert_eq!(results.get(2).unwrap(), BatchResolveResult::Ok(addr2));
    }

    #[test]
    fn test_batch_resolve_increments_total_routed() {
        let (env, admin, client) = setup();
        let oracle = String::from_str(&env, "oracle");
        let vault = String::from_str(&env, "vault");
        let addr1 = Address::generate(&env);
        let addr2 = Address::generate(&env);
        client.register_route(&admin, &oracle, &addr1, &None);
        client.register_route(&admin, &vault, &addr2, &None);

        assert_eq!(client.total_routed(), 0);
        let names = vec![&env, oracle, vault];
        client.batch_resolve(&names);
        assert_eq!(client.total_routed(), 2);
    }

    #[test]
    fn test_batch_resolve_resolves_alias() {
        let (env, admin, client) = setup();
        let name = String::from_str(&env, "oracle");
        let alias = String::from_str(&env, "oracle-v1");
        let addr = Address::generate(&env);
        client.register_route(&admin, &name, &addr, &None);
        client.add_alias(&admin, &name, &alias);

        let names = vec![&env, alias];
        let results = client.batch_resolve(&names);

        assert_eq!(results.len(), 1);
        assert_eq!(results.get(0).unwrap(), BatchResolveResult::Ok(addr));
    }
}

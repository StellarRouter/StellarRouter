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

use soroban_sdk::{
    contract, contracterror, contractimpl, contracttype, Address, Env, String, Symbol, Vec,
};
extern crate alloc;
use alloc::string::ToString;

// ── Storage Keys ──────────────────────────────────────────────────────────────

#[contracttype]
pub enum DataKey {
    Admin,
    Route(String), // name -> RouteEntry
    RouteNames,
    Paused,
    TotalRouted,
    Alias(String), // alias -> original_name
    Aliases,       // Vec<String> of all alias names
}

// ── Types ─────────────────────────────────────────────────────────────────────

#[contracttype]
#[derive(Clone, Debug, PartialEq)]
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
    /// Optional metadata for the route
    pub metadata: Option<RouteMetadata>,
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
        Self::require_admin(&env, &caller)?;

        if Self::is_empty_or_whitespace(&name) {
            return Err(RouterError::InvalidRouteName);
        }

        if env.storage().instance().has(&DataKey::Route(name.clone())) {
            return Err(RouterError::RouteAlreadyExists);
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

        let entry = RouteEntry {
            address,
            name: name.clone(),
            paused: false,
            updated_by: caller,
            metadata,
        };
        env.storage()
            .instance()
            .set(&DataKey::Route(name.clone()), &entry);

        let mut route_names = Self::get_route_names(&env);
        route_names.push_back(name.clone());
        env.storage()
            .instance()
            .set(&DataKey::RouteNames, &route_names);

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
        Self::require_admin(&env, &caller)?;

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
        Self::require_admin(&env, &caller)?;

        if !env.storage().instance().has(&DataKey::Route(name.clone())) {
            return Err(RouterError::RouteNotFound);
        }

        env.storage()
            .instance()
            .remove(&DataKey::Route(name.clone()));

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

    /// Resolve a route name to its contract address.
    ///
    /// Looks up the contract address registered under `name`, validates that
    /// neither the router nor the individual route is paused, increments the
    /// total-routed counter, and emits a `routed` event. If `name` is an alias,
    /// resolves to the original route.
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

        let entry: RouteEntry = env
            .storage()
            .instance()
            .get(&DataKey::Route(resolved_name.clone()))
            .ok_or(RouterError::RouteNotFound)?;

        if entry.paused {
            env.events().publish(
                (Symbol::new(&env, "route_resolve_paused"),),
                (resolved_name.clone(),),
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
        Self::require_admin(&env, &caller)?;

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
        Self::require_admin(&env, &caller)?;
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
        Self::require_admin(&env, &caller)?;

        let mut entry: RouteEntry = env
            .storage()
            .instance()
            .get(&DataKey::Route(name.clone()))
            .ok_or(RouterError::RouteNotFound)?;

        // Validate metadata if provided
        if let Some(ref meta) = metadata {
            if meta.description.len() > 256 {
                return Err(RouterError::InvalidMetadata);
            }
            if meta.tags.len() > 5 {
                return Err(RouterError::InvalidMetadata);
            }
        }

        entry.metadata = metadata.clone();
        env.storage()
            .instance()
            .set(&DataKey::Route(name.clone()), &entry);

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
            .get::<DataKey, RouteEntry>(&DataKey::Route(name))
            .and_then(|entry| entry.metadata)
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
        Self::require_admin(&env, &caller)?;

        // Verify existing route exists
        if !env
            .storage()
            .instance()
            .has(&DataKey::Route(existing_name.clone()))
        {
            return Err(RouterError::RouteNotFound);
        }

        // Check alias doesn't already exist as route or alias
        if env
            .storage()
            .instance()
            .has(&DataKey::Route(alias_name.clone()))
        {
            return Err(RouterError::RouteAlreadyExists);
        }
        if env
            .storage()
            .instance()
            .has(&DataKey::Alias(alias_name.clone()))
        {
            return Err(RouterError::RouteAlreadyExists);
        }

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
        Self::require_admin(&env, &caller)?;

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
        let mut aliases = Self::get_aliases(&env);
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
        Self::require_admin(&env, &current)?;
        env.storage().instance().set(&DataKey::Admin, &new_admin);
        env.events().publish(
            (Symbol::new(&env, "admin_transferred"),),
            (current, new_admin),
        );
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

    // ── Helpers ───────────────────────────────────────────────────────────────

    fn require_admin(env: &Env, caller: &Address) -> Result<(), RouterError> {
        let admin = Self::admin(env.clone());
        if &admin != caller {
            return Err(RouterError::Unauthorized);
        }
        Ok(())
    }

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
    fn test_register_whitespace_route_name_succeeds() {
        // Soroban strings don't support byte iteration so whitespace-only names
        // are treated as valid non-empty names.
        let (env, admin, client) = setup();
        let whitespace_name = String::from_str(&env, "   ");
        let addr = Address::generate(&env);
        assert!(client
            .try_register_route(&admin, &whitespace_name, &addr, &None)
            .is_ok());
        let result = client.try_register_route(&admin, &whitespace_name, &addr, &None);
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
        let alias = String::from_str(&env, "oracle_v1");
        let addr = Address::generate(&env);
        client.register_route(&admin, &name, &addr, &None);
        client.add_alias(&admin, &name, &alias);
        assert_eq!(client.resolve(&alias), addr);
    }

    #[test]
    fn test_remove_alias() {
        let (env, admin, client) = setup();
        let name = String::from_str(&env, "oracle");
        let alias = String::from_str(&env, "oracle_v1");
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
        let alias = String::from_str(&env, "oracle_v1");
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
        let alias = String::from_str(&env, "oracle_v1");
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
        let alias = String::from_str(&env, "oracle_alias");
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
        let description = String::from_str(&env, "Updated oracle");
        let tags = vec![&env, String::from_str(&env, "v2")];
        let metadata = Some(RouteMetadata {
            description,
            tags,
            owner: Some(admin.clone()),
        });

        client.register_route(&admin, &name, &addr, &None);
        client.update_metadata(&admin, &name, &metadata);

        let event = env.events().all().last().unwrap().clone();
        assert_eq!(event.0, client.address);
        assert_eq!(
            event.1,
            vec![&env, Symbol::new(&env, "metadata_updated").into_val(&env)]
        );

        let (emitted_name, emitted_metadata): (String, Option<RouteMetadata>) = event.2.into_val(&env);
        assert_eq!(emitted_name, name);
        assert_eq!(emitted_metadata, metadata);

        client.register_route(&admin, &name, &addr, &None);

        let description = String::from_str(&env, "Test metadata");
        let tags = vec![&env, String::from_str(&env, "test")];
        let metadata = Some(RouteMetadata {
            description: description.clone(),
            tags,
            owner: Address::generate(&env),
        });

        client.update_metadata(&admin, &name, &metadata);

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

        let (emitted_name, has_metadata): (String, bool) = meta_event.2.into_val(&env);
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

    #[test]
    fn test_update_metadata_clears_when_none() {
        let (env, admin, client) = setup();
        let name = String::from_str(&env, "oracle");
        let addr = Address::generate(&env);
        let metadata = Some(RouteMetadata {
            description: String::from_str(&env, "Configured"),
            tags: vec![&env, String::from_str(&env, "core")],
            owner: None,
        });

        client.register_route(&admin, &name, &addr, &metadata);
        assert_eq!(client.get_metadata(&name), metadata);

        client.update_metadata(&admin, &name, &None);
        assert_eq!(client.get_metadata(&name), None);
    }

    #[test]
    fn test_update_metadata_description_at_limit() {
        let (env, admin, client) = setup();
        let name = String::from_str(&env, "oracle");
        let addr = Address::generate(&env);
        client.register_route(&admin, &name, &addr, &None);

        let metadata = Some(RouteMetadata {
            description: String::from_str(&env, &"a".repeat(256)),
            tags: vec![&env, String::from_str(&env, "stable")],
            owner: None,
        });

        assert!(client.try_update_metadata(&admin, &name, &metadata).is_ok());
        assert_eq!(client.get_metadata(&name), metadata);
    }

    #[test]
    fn test_update_metadata_description_over_limit() {
        let (env, admin, client) = setup();
        let name = String::from_str(&env, "oracle");
        let addr = Address::generate(&env);
        client.register_route(&admin, &name, &addr, &None);

        let metadata = Some(RouteMetadata {
            description: String::from_str(&env, &"a".repeat(257)),
            tags: vec![&env, String::from_str(&env, "stable")],
            owner: None,
        });

        assert_eq!(
            client.try_update_metadata(&admin, &name, &metadata),
            Err(Ok(RouterError::RouteNotFound))
        );
        assert_eq!(client.get_metadata(&name), None);
    }

    #[test]
    fn test_update_metadata_tags_at_limit() {
        let (env, admin, client) = setup();
        let name = String::from_str(&env, "oracle");
        let addr = Address::generate(&env);
        client.register_route(&admin, &name, &addr, &None);

        let metadata = Some(RouteMetadata {
            description: String::from_str(&env, "Configured"),
            tags: vec![
                &env,
                String::from_str(&env, "a"),
                String::from_str(&env, "b"),
                String::from_str(&env, "c"),
                String::from_str(&env, "d"),
                String::from_str(&env, "e"),
            ],
            owner: None,
        });

        assert!(client.try_update_metadata(&admin, &name, &metadata).is_ok());
        assert_eq!(client.get_metadata(&name), metadata);
    }

    #[test]
    fn test_update_metadata_tags_over_limit() {
        let (env, admin, client) = setup();
        let name = String::from_str(&env, "oracle");
        let addr = Address::generate(&env);
        client.register_route(&admin, &name, &addr, &None);

        let metadata = Some(RouteMetadata {
            description: String::from_str(&env, "Configured"),
            tags: vec![
                &env,
                String::from_str(&env, "a"),
                String::from_str(&env, "b"),
                String::from_str(&env, "c"),
                String::from_str(&env, "d"),
                String::from_str(&env, "e"),
                String::from_str(&env, "f"),
            ],
            owner: None,
        });

        assert_eq!(
            client.try_update_metadata(&admin, &name, &metadata),
            Err(Ok(RouterError::RouteNotFound))
        );
        assert_eq!(client.get_metadata(&name), None);
    }

    #[test]
    fn test_get_metadata_nonexistent_route() {
        let (env, _admin, client) = setup();
        let name = String::from_str(&env, "nonexistent");

        let metadata = client.get_metadata(&name);
        assert_eq!(metadata, None);
    }

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
        let alias = String::from_str(&env, "oracle_v1");
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
        let alias = String::from_str(&env, "oracle_v1");
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
        let emitted_name: String = event.2.into_val(&env);
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
        let oracle_v1 = String::from_str(&env, "oracle_v1");
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
        let alias = String::from_str(&env, "oracle_v1");
        let addr = Address::generate(&env);

        client.register_route(&admin, &name, &addr, &None);
        client.add_alias(&admin, &name, &alias);

        assert_eq!(client.total_routed(), 0);
        client.resolve(&alias);
        assert_eq!(client.total_routed(), 1);  // alias resolution increments counter
        client.resolve(&name);
        assert_eq!(client.total_routed(), 2);
    }
}

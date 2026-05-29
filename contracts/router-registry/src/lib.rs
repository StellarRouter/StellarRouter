#![no_std]

//! # router-registry
//!
//! Central registry for the stellar-router suite.
//! Stores contract addresses keyed by name + version, supports deprecation and lookup.
//!
//! ## Features
//! - Register contracts by name and semantic version
//! - Lookup latest or specific version of a contract
//! - Deprecate old versions
//! - Admin-controlled with ownership transfer
//!
//! ## Events (following naming convention: past tense verbs in snake_case)
//! - `contract_registered` — Contract registered (contract_name, version)
//! - `contract_deprecated` — Contract version deprecated (contract_name, version)
//! - `admin_transferred` — Admin transferred (old_admin, new_admin)

extern crate alloc;
use alloc::string::ToString;
use soroban_sdk::{
    contract, contracterror, contractimpl, contracttype, Address, Env, String, Symbol, Vec,
};

// ── Storage Keys ──────────────────────────────────────────────────────────────

#[contracttype]
pub enum DataKey {
    Admin,
    Entry(String, u32),    // (name, version) -> ContractEntry
    Versions(String),      // name -> Vec<u32>
    ContractNames,         // Vec<String> of all registered names
    AddressIndex(Address), // address -> (name, version) for O(1) reverse lookup
    LatestVersion(String), // name -> u32: cached latest non-deprecated version
}

// ── Types ─────────────────────────────────────────────────────────────────────

#[contracttype]
#[derive(Clone, Debug, PartialEq)]
pub struct ContractEntry {
    /// Registered contract address
    pub address: Address,
    /// Human-readable name
    pub name: String,
    /// Version number (monotonically increasing)
    pub version: u32,
    /// Whether this entry has been deprecated
    pub deprecated: bool,
    /// Optional human-readable reason for deprecation
    pub deprecation_reason: Option<String>,
    /// Who registered it
    pub registered_by: Address,
}

// ── Errors ────────────────────────────────────────────────────────────────────

#[contracterror]
#[derive(Copy, Clone, Debug, PartialEq)]
pub enum RegistryError {
    AlreadyInitialized = 1,
    NotInitialized = 2,
    Unauthorized = 3,
    NotFound = 4,
    AlreadyRegistered = 5,
    AlreadyDeprecated = 6,
    InvalidVersion = 7,
    VersionNotFound = 8,
    InvalidConstraint = 9,
    AllVersionsDeprecated = 10,
}

// ── Contract ──────────────────────────────────────────────────────────────────

#[contract]
pub struct RouterRegistry;

#[contractimpl]
impl RouterRegistry {
    /// Initialize the registry with an admin address.
    ///
    /// Must be called exactly once before any other function. Sets the admin
    /// who controls all write operations on the registry.
    ///
    /// # Arguments
    /// * `env` - The Soroban environment.
    /// * `admin` - The address that will have admin privileges over this registry.
    ///
    /// # Returns
    /// `Ok(())` on success.
    ///
    /// # Errors
    /// * [`RegistryError::AlreadyInitialized`] — if the contract has already been initialized.
    pub fn initialize(env: Env, admin: Address) -> Result<(), RegistryError> {
        if env.storage().instance().has(&DataKey::Admin) {
            return Err(RegistryError::AlreadyInitialized);
        }
        env.storage().instance().set(&DataKey::Admin, &admin);
        Ok(())
    }

    /// Register a new contract entry.
    ///
    /// Stores a [`ContractEntry`] keyed by `(name, version)`. The `version`
    /// must be greater than all previously registered versions for the same
    /// `name` and must be non-zero. Caller must be the admin.
    ///
    /// # Arguments
    /// * `env` - The Soroban environment.
    /// * `caller` - The address initiating the call; must be the admin.
    /// * `name` - A human-readable identifier for the contract.
    /// * `address` - The contract address to register.
    /// * `version` - A monotonically increasing version number (must be > 0 and
    ///   greater than any existing version for `name`).
    ///
    /// # Returns
    /// `Ok(())` on success.
    ///
    /// # Errors
    /// * [`RegistryError::Unauthorized`] — if `caller` is not the admin.
    /// * [`RegistryError::InvalidVersion`] — if `version` is 0 or not greater than all existing versions.
    /// * [`RegistryError::AlreadyRegistered`] — if `(name, version)` is already registered.
    /// * [`RegistryError::NotInitialized`] — if the contract has not been initialized.
    pub fn register(
        env: Env,
        caller: Address,
        name: String,
        address: Address,
        version: u32,
    ) -> Result<(), RegistryError> {
        caller.require_auth();
        router_common::require_admin_simple!(&env, &caller, &DataKey::Admin, RegistryError)?;

        if version == 0 {
            return Err(RegistryError::InvalidVersion);
        }

        if env
            .storage()
            .instance()
            .has(&DataKey::Entry(name.clone(), version))
        {
            return Err(RegistryError::AlreadyRegistered);
        }

        // Validate version is greater than all existing versions
        let versions = Self::get_versions_list(&env, &name);
        for v in versions.iter() {
            if version <= v {
                return Err(RegistryError::InvalidVersion);
            }
        }

        let entry = ContractEntry {
            address,
            name: name.clone(),
            version,
            deprecated: false,
            deprecation_reason: None,
            registered_by: caller,
        };

        env.storage()
            .instance()
            .set(&DataKey::Entry(name.clone(), version), &entry);

        // Update address index
        env.storage()
            .instance()
            .set(&DataKey::AddressIndex(address.clone()), &(name.clone(), version));

        // Update version list
        let mut versions = Self::get_versions_list(&env, &name);
        versions.push_back(version);
        env.storage()
            .instance()
            .set(&DataKey::Versions(name.clone()), &versions);

        // Update contract names list
        let mut names: Vec<String> = env
            .storage()
            .instance()
            .get(&DataKey::ContractNames)
            .unwrap_or_else(|| Vec::new(&env));
        if !names.contains(&name) {
            names.push_back(name.clone());
            env.storage()
                .instance()
                .set(&DataKey::ContractNames, &names);
        }

        // Update address index for O(1) reverse lookup
        env.storage()
            .instance()
            .set(&DataKey::AddressIndex(entry.address.clone()), &(name.clone(), version));

        // New version is always the highest (validated above), so it becomes the new latest.
        env.storage()
            .instance()
            .set(&DataKey::LatestVersion(name.clone()), &version);

        env.events()
            .publish((Symbol::new(&env, "contract_registered"),), (name, version, None::<String>));

        Ok(())
    }

    /// Register multiple contract entries atomically.
    ///
    /// Validates and registers all entries in `entries`. If any entry fails
    /// validation or registration, the function returns immediately with that
    /// error and no further entries are registered (fail-fast / roll-back on
    /// first failure).
    ///
    /// # Arguments
    /// * `env` - The Soroban environment.
    /// * `caller` - The address initiating the call; must be the admin.
    /// * `entries` - A list of `(name, address, version)` tuples to register.
    ///
    /// # Returns
    /// `Ok(())` if all entries were registered successfully.
    ///
    /// # Errors
    /// * [`RegistryError::Unauthorized`] — if `caller` is not the admin.
    /// * [`RegistryError::InvalidVersion`] — if any version is 0 or not monotonically increasing.
    /// * [`RegistryError::AlreadyRegistered`] — if any `(name, version)` is already registered.
    /// * [`RegistryError::NotInitialized`] — if the contract has not been initialized.
    pub fn bulk_register(
        env: Env,
        caller: Address,
        entries: Vec<(String, Address, u32)>,
    ) -> Result<(), RegistryError> {
        caller.require_auth();
        Self::require_admin(&env, &caller)?;

        for (name, address, version) in entries.iter() {
            if version == 0 {
                return Err(RegistryError::InvalidVersion);
            }
            if env
                .storage()
                .instance()
                .has(&DataKey::Entry(name.clone(), version))
            {
                return Err(RegistryError::AlreadyRegistered);
            }
            let versions = Self::get_versions_list(&env, &name);
            for v in versions.iter() {
                if version <= v {
                    return Err(RegistryError::InvalidVersion);
                }
            }

            let entry = ContractEntry {
                address: address.clone(),
                name: name.clone(),
                version,
                deprecated: false,
                deprecation_reason: None,
                registered_by: caller.clone(),
            };

            env.storage()
                .instance()
                .set(&DataKey::Entry(name.clone(), version), &entry);

            env.storage()
                .instance()
                .set(&DataKey::AddressIndex(address.clone()), &(name.clone(), version));

            let mut versions = Self::get_versions_list(&env, &name);
            versions.push_back(version);
            env.storage()
                .instance()
                .set(&DataKey::Versions(name.clone()), &versions);

            let mut names: Vec<String> = env
                .storage()
                .instance()
                .get(&DataKey::ContractNames)
                .unwrap_or_else(|| Vec::new(&env));
            if !names.contains(&name) {
                names.push_back(name.clone());
                env.storage()
                    .instance()
                    .set(&DataKey::ContractNames, &names);
            }

            env.events().publish(
                (Symbol::new(&env, "contract_registered"),),
                (name.clone(), version, None::<String>),
            );
        }

        Ok(())
    }

    /// Look up a contract by name and specific version.
    ///
    /// # Arguments
    /// * `env` - The Soroban environment.
    /// * `name` - The human-readable name of the contract.
    /// * `version` - The exact version number to retrieve.
    ///
    /// # Returns
    /// The [`ContractEntry`] for `(name, version)`.
    ///
    /// # Errors
    /// * [`RegistryError::NotFound`] — if no entry exists for `(name, version)`.
    pub fn get(env: Env, name: String, version: u32) -> Result<ContractEntry, RegistryError> {
        env.storage()
            .instance()
            .get(&DataKey::Entry(name, version))
            .ok_or(RegistryError::NotFound)
    }

    /// Get the latest (highest version) non-deprecated entry for a name.
    ///
    /// Iterates registered versions in descending order and returns the first
    /// entry that has not been deprecated.
    ///
    /// # Arguments
    /// * `env` - The Soroban environment.
    /// * `name` - The human-readable name of the contract.
    ///
    /// # Returns
    /// The most recent non-deprecated [`ContractEntry`] for `name`.
    ///
    /// # Errors
    /// * [`RegistryError::NotFound`] — if no non-deprecated entry exists for `name`.
    pub fn get_latest(env: Env, name: String) -> Result<ContractEntry, RegistryError> {
        let version: u32 = env
            .storage()
            .instance()
            .get(&DataKey::LatestVersion(name.clone()))
            .ok_or(RegistryError::NotFound)?;
        env.storage()
            .instance()
            .get(&DataKey::Entry(name, version))
            .ok_or(RegistryError::AllVersionsDeprecated)
    }

    /// Get the latest non-deprecated entry matching a semver constraint.
    ///
    /// Accepts an optional semver constraint string (e.g., ">=2.0,<3.0" or "^1.5").
    /// Returns the highest non-deprecated version satisfying the constraint.
    ///
    /// # Arguments
    /// * `env` - The Soroban environment.
    /// * `name` - The human-readable name of the contract.
    /// * `constraint` - Optional semver constraint (e.g., ">=2.0,<3.0").
    ///
    /// # Returns
    /// The most recent non-deprecated [`ContractEntry`] matching the constraint.
    ///
    /// # Errors
    /// * [`RegistryError::NotFound`] — if no matching entry exists.
    /// * [`RegistryError::InvalidConstraint`] — if constraint format is invalid.
    pub fn get_latest_with_constraint(
        env: Env,
        name: String,
        constraint: Option<String>,
    ) -> Result<ContractEntry, RegistryError> {
        let versions = Self::get_versions_list(&env, &name);

        // If no constraint, delegate to get_latest (uses cache).
        if constraint.is_none() {
            return Self::get_latest(env.clone(), name);
        }

        let constraint_str = constraint.unwrap();
        let mut matched_constraint = false;

        if versions.is_empty() {
            return Err(RegistryError::NotFound);
        }

        let mut any_constraint_match = false;

        // Iterate in reverse to find latest matching non-deprecated version
        let len = versions.len();
        let mut i = len;
        while i > 0 {
            i -= 1;
            let v = versions.get(i).ok_or(RegistryError::NotFound)?;
            let entry: ContractEntry = env
                .storage()
                .instance()
                .get(&DataKey::Entry(name.clone(), v))
                .ok_or(RegistryError::NotFound)?;
            if Self::version_matches_constraint(v, &constraint_str)? {
                matched_constraint = true;
            } else {
                continue;
            }

            if !entry.deprecated {
                return Ok(entry);
            }
        }
        if any_constraint_match {
            Err(RegistryError::AllVersionsDeprecated)
        } else {
            Err(RegistryError::NotFound)
        }
    }

    /// Deprecate a specific version of a contract.
    ///
    /// Marks the entry for `(name, version)` as deprecated so it will be
    /// skipped by `get_latest`. Caller must be the admin.
    ///
    /// # Arguments
    /// * `env` - The Soroban environment.
    /// * `caller` - The address initiating the call; must be the admin.
    /// * `name` - The human-readable name of the contract.
    /// * `version` - The version number to deprecate.
    /// * `reason` - Optional human-readable reason for the deprecation.
    ///
    /// # Returns
    /// `Ok(())` on success.
    ///
    /// # Errors
    /// * [`RegistryError::Unauthorized`] — if `caller` is not the admin.
    /// * [`RegistryError::NotFound`] — if no entry exists for `(name, version)`.
    /// * [`RegistryError::AlreadyDeprecated`] — if the entry is already deprecated.
    /// * [`RegistryError::NotInitialized`] — if the contract has not been initialized.
    pub fn deprecate(
        env: Env,
        caller: Address,
        name: String,
        version: u32,
        reason: Option<String>,
    ) -> Result<(), RegistryError> {
        caller.require_auth();
        router_common::require_admin_simple!(&env, &caller, &DataKey::Admin, RegistryError)?;
        Self::deprecate_one(&env, name, version, reason)
    }

    fn deprecate_one(
        env: &Env,
        name: String,
        version: u32,
        reason: Option<String>,
    ) -> Result<(), RegistryError> {
        let mut entry: ContractEntry = env
            .storage()
            .instance()
            .get(&DataKey::Entry(name.clone(), version))
            .ok_or(RegistryError::VersionNotFound)?;

        if entry.deprecated {
            return Err(RegistryError::AlreadyDeprecated);
        }

        entry.deprecated = true;
        entry.deprecation_reason = reason.clone();
        env.storage()
            .instance()
            .set(&DataKey::Entry(name.clone(), version), &entry);

        // Remove address index so deprecated entries are no longer reachable by address
        env.storage()
            .instance()
            .remove(&DataKey::AddressIndex(entry.address.clone()));

        // Update LatestVersion cache: scan versions in reverse for new latest non-deprecated.
        let versions = Self::get_versions_list(env, &name);
        let len = versions.len();
        let mut new_latest: Option<u32> = None;
        let mut i = len;
        while i > 0 {
            i -= 1;
            let v = versions.get(i).unwrap();
            let e: ContractEntry = env
                .storage()
                .instance()
                .get(&DataKey::Entry(name.clone(), v))
                .unwrap();
            if !e.deprecated {
                new_latest = Some(v);
                break;
            }
        }
        match new_latest {
            Some(v) => env
                .storage()
                .instance()
                .set(&DataKey::LatestVersion(name.clone()), &v),
            None => env
                .storage()
                .instance()
                .remove(&DataKey::LatestVersion(name.clone())),
        }

        env.events().publish(
            (Symbol::new(&env, "contract_deprecated"),),
            (name, version, reason),
        );
        Ok(())
    }

    pub fn deprecate_many(
        env: Env,
        caller: Address,
        entries: Vec<(String, u32)>,
    ) -> Result<Vec<Result<(), RegistryError>>, RegistryError> {
        caller.require_auth();
        router_common::require_admin_simple!(&env, &caller, &DataKey::Admin, RegistryError)?;
        let mut results = Vec::new(&env);
        for (name, version) in entries.iter() {
            results.push_back(Self::deprecate_one(&env, name, version, None));
        }
        Ok(results)
    }

    /// Deprecate all registered versions of a contract in one call.
    ///
    /// Iterates every version registered under `name` and marks each as
    /// deprecated. Versions that are already deprecated are silently skipped.
    /// Caller must be the admin.
    ///
    /// # Errors
    /// * [`RegistryError::Unauthorized`] — if `caller` is not the admin.
    /// * [`RegistryError::NotInitialized`] — if the contract has not been initialized.
    pub fn deprecate_all_versions(
        env: Env,
        caller: Address,
        name: String,
        reason: Option<String>,
    ) -> Result<(), RegistryError> {
        caller.require_auth();
        router_common::require_admin_simple!(&env, &caller, &DataKey::Admin, RegistryError)?;
        let versions = Self::get_versions_list(&env, &name);
        for v in versions.iter() {
            // Skip already-deprecated entries rather than aborting
            let _ = Self::deprecate_one(&env, name.clone(), v, reason.clone());
        }
        Ok(())
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
    /// * [`RegistryError::Unauthorized`] — if `current` is not the admin.
    /// * [`RegistryError::NotInitialized`] — if the contract has not been initialized.
    pub fn transfer_admin(
        env: Env,
        current: Address,
        new_admin: Address,
    ) -> Result<(), RegistryError> {
        current.require_auth();
        router_common::require_admin_simple!(&env, &current, &DataKey::Admin, RegistryError)?;
        router_common::admin_transfer_complete!(&env, &current, &new_admin, &DataKey::Admin);
        Ok(())
    }

    /// Get the current admin.
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
    /// Get the current admin address.
    ///
    /// # Errors
    /// Returns `RegistryError::NotInitialized` if the contract has not been initialized.
    pub fn admin(env: Env) -> Result<Address, RegistryError> {
        env.storage()
            .instance()
            .get(&DataKey::Admin)
            .ok_or(RegistryError::NotInitialized)
    }

    /// Get all registered versions for a name.
    ///
    /// Returns the list of version numbers that have been registered under
    /// `name`, in the order they were registered (ascending).
    ///
    /// # Arguments
    /// * `env` - The Soroban environment.
    /// * `name` - The human-readable name of the contract.
    ///
    /// # Returns
    /// A [`Vec<u32>`] of version numbers. Returns an empty vector if `name`
    /// has no registered versions.
    pub fn versions(env: Env, name: String) -> Vec<u32> {
        Self::get_versions_list(&env, &name)
    }

    /// Get all contract entries for a name.
    ///
    /// Returns all [`ContractEntry`] structs for `name`, including deprecated ones,
    /// in version order (ascending).
    ///
    /// # Arguments
    /// * `env` - The Soroban environment.
    /// * `name` - The human-readable name of the contract.
    ///
    /// # Returns
    /// A [`Vec<ContractEntry>`] of all entries for `name`.
    pub fn get_all_versions(env: Env, name: String) -> Vec<ContractEntry> {
        let versions = Self::get_versions_list(&env, &name);
        let mut entries = Vec::new(&env);
        for v in versions.iter() {
            if let Some(entry) = env
                .storage()
                .instance()
                .get(&DataKey::Entry(name.clone(), v))
            {
                entries.push_back(entry);
            }
        }
        entries
    }

    /// Returns all registered contract names in registration order.
    ///
    /// Returns a list of all unique contract names that have been registered,
    /// in the order they were first registered.
    ///
    /// # Arguments
    /// * `env` - The Soroban environment.
    ///
    /// # Returns
    /// A [`Vec<String>`] of all registered contract names.
    pub fn get_all_names(env: Env) -> Vec<String> {
        env.storage()
            .instance()
            .get(&DataKey::ContractNames)
            .unwrap_or_else(|| Vec::new(&env))
    }

    /// Find the registry entry for a given contract address (O(1) reverse lookup).
    ///
    /// Uses a direct `AddressIndex` lookup — O(1) — instead of scanning all entries.
    /// Returns `None` if the address is not registered or has been deprecated.
    /// Uses the address index written by `register()` to look up the entry
    /// in constant time. Returns `None` if the address is not registered.
    ///
    /// # Arguments
    /// * `env` - The Soroban environment.
    /// * `address` - The contract address to look up.
    ///
    /// # Returns
    /// An [`Option<ContractEntry>`] containing the entry if found, `None` otherwise.
    pub fn get_entry_by_address(env: Env, address: Address) -> Option<ContractEntry> {
        let (name, version): (String, u32) = env
            .storage()
            .instance()
            .get(&DataKey::AddressIndex(address))?;
        let (name, version) = env
            .storage()
            .instance()
            .get::<DataKey, (String, u32)>(&DataKey::AddressIndex(address))?;
        env.storage()
            .instance()
            .get(&DataKey::Entry(name, version))
    }

    // ── Helpers ───────────────────────────────────────────────────────────────

    fn get_versions_list(env: &Env, name: &String) -> Vec<u32> {
        env.storage()
            .instance()
            .get(&DataKey::Versions(name.clone()))
            .unwrap_or(Vec::new(env))
    }

    fn version_matches_constraint(
        version: u32,
        constraint: &String,
    ) -> Result<bool, RegistryError> {
        // Parse simple semver constraints: >=X, <=X, >X, <X, ^X, ~X
        let constraint_str = constraint.to_string();

        if constraint_str.starts_with(">=") {
            let min = constraint_str[2..]
                .parse::<u32>()
                .map_err(|_| RegistryError::InvalidConstraint)?;
            Ok(version >= min)
        } else if constraint_str.starts_with("<=") {
            let max = constraint_str[2..]
                .parse::<u32>()
                .map_err(|_| RegistryError::InvalidConstraint)?;
            Ok(version <= max)
        } else if constraint_str.starts_with(">") {
            let min = constraint_str[1..]
                .parse::<u32>()
                .map_err(|_| RegistryError::InvalidConstraint)?;
            Ok(version > min)
        } else if constraint_str.starts_with("<") {
            let max = constraint_str[1..]
                .parse::<u32>()
                .map_err(|_| RegistryError::InvalidConstraint)?;
            Ok(version < max)
        } else if constraint_str.starts_with("^") {
            // Caret: allows changes that do not modify the left-most non-zero digit
            let base = constraint_str[1..]
                .parse::<u32>()
                .map_err(|_| RegistryError::InvalidConstraint)?;
            if base == 0 {
                Ok(version >= base && version < 1)
            } else {
                Ok(version >= base && version < base + 1)
            }
        } else if constraint_str.starts_with("~") {
            // Tilde: allows patch-level changes
            let base = constraint_str[1..]
                .parse::<u32>()
                .map_err(|_| RegistryError::InvalidConstraint)?;
            Ok(version >= base && version < base + 1)
        } else {
            // Try exact match
            let exact = constraint_str
                .parse::<u32>()
                .map_err(|_| RegistryError::InvalidConstraint)?;
            Ok(version == exact)
        }
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

    fn setup() -> (Env, Address, RouterRegistryClient<'static>) {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register_contract(None, RouterRegistry);
        let client = RouterRegistryClient::new(&env, &contract_id);
        let admin = Address::generate(&env);
        client.initialize(&admin);
        (env, admin, client)
    }

    #[test]
    fn test_initialize() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register_contract(None, RouterRegistry);
        let client = RouterRegistryClient::new(&env, &contract_id);
        let admin = Address::generate(&env);
        let result = client.try_initialize(&admin);
        assert!(result.is_ok());
    }

    #[test]
    fn test_double_initialize_fails() {
        let (_, admin, client) = setup();
        let result = client.try_initialize(&admin);
        assert_eq!(result, Err(Ok(RegistryError::AlreadyInitialized)));
    }

    #[test]
    fn test_register_and_get() {
        let (env, admin, client) = setup();
        let name = String::from_str(&env, "oracle");
        let addr = Address::generate(&env);
        client.register(&admin, &name, &addr, &1);
        let entry = client.get(&name, &1);
        assert_eq!(entry.address, addr);
        assert_eq!(entry.version, 1);
        assert!(!entry.deprecated);
    }

    #[test]
    fn test_get_latest() {
        let (env, admin, client) = setup();
        let name = String::from_str(&env, "oracle");
        let addr1 = Address::generate(&env);
        let addr2 = Address::generate(&env);
        client.register(&admin, &name, &addr1, &1);
        client.register(&admin, &name, &addr2, &2);
        let latest = client.get_latest(&name);
        assert_eq!(latest.address, addr2);
        assert_eq!(latest.version, 2);
    }

    #[test]
    fn test_deprecate() {
        let (env, admin, client) = setup();
        let name = String::from_str(&env, "oracle");
        let addr1 = Address::generate(&env);
        let addr2 = Address::generate(&env);
        client.register(&admin, &name, &addr1, &1);
        client.register(&admin, &name, &addr2, &2);
        client.deprecate(&admin, &name, &2, &None);
        client.deprecate(&admin, &name, &2, &None::<String>);
        // latest should now return v1
        let latest = client.get_latest(&name);
        assert_eq!(latest.version, 1);
    }

    #[test]
    fn test_deprecate_nonexistent_version_returns_error() {
        let (env, admin, client) = setup();
        let name = String::from_str(&env, "oracle");
        let addr = Address::generate(&env);
        client.register(&admin, &name, &addr, &1);
        let result = client.try_deprecate(&admin, &name, &99, &None);
        let result = client.try_deprecate(&admin, &name, &99, &None::<String>);
        assert_eq!(result, Err(Ok(RegistryError::VersionNotFound)));
    }

    #[test]
    fn test_get_latest_unknown_name_returns_not_found() {
        let (env, _admin, client) = setup();
        let name = String::from_str(&env, "unknown");
        let result = client.try_get_latest(&name);
        assert_eq!(result, Err(Ok(RegistryError::NotFound)));
    }

    #[test]
    fn test_get_latest_returns_not_found_when_all_deprecated() {
        let (env, admin, client) = setup();
        let name = String::from_str(&env, "oracle");
        let addr = Address::generate(&env);
        client.register(&admin, &name, &addr, &1);
        client.deprecate(&admin, &name, &1, &None);
        client.deprecate(&admin, &name, &1, &None::<String>);
        let result = client.try_get_latest(&name);
        assert_eq!(result, Err(Ok(RegistryError::AllVersionsDeprecated)));
    }

    #[test]
    fn test_get_latest_skips_multiple_deprecated_versions() {
        let (env, admin, client) = setup();
        let name = String::from_str(&env, "oracle");
        let (a1, a2, a3) = (
            Address::generate(&env),
            Address::generate(&env),
            Address::generate(&env),
        );
        client.register(&admin, &name, &a1, &1);
        client.register(&admin, &name, &a2, &2);
        client.register(&admin, &name, &a3, &3);
        client.deprecate(&admin, &name, &3, &None);
        client.deprecate(&admin, &name, &2, &None);
        client.deprecate(&admin, &name, &3, &None::<String>);
        client.deprecate(&admin, &name, &2, &None::<String>);
        let latest = client.get_latest(&name);
        assert_eq!(latest.version, 1);
    }

    #[test]
    fn test_duplicate_version_fails() {
        let (env, admin, client) = setup();
        let name = String::from_str(&env, "oracle");
        let addr = Address::generate(&env);
        client.register(&admin, &name, &addr, &1);
        let result = client.try_register(&admin, &name, &addr, &1);
        assert_eq!(result, Err(Ok(RegistryError::AlreadyRegistered)));
    }

    #[test]
    fn test_version_must_increase() {
        let (env, admin, client) = setup();
        let name = String::from_str(&env, "oracle");
        let addr = Address::generate(&env);
        client.register(&admin, &name, &addr, &5);
        let result = client.try_register(&admin, &name, &addr, &3);
        assert_eq!(result, Err(Ok(RegistryError::InvalidVersion)));
    }

    #[test]
    fn test_unauthorized_register_fails() {
        let (env, _admin, client) = setup();
        let name = String::from_str(&env, "oracle");
        let addr = Address::generate(&env);
        let attacker = Address::generate(&env);
        let result = client.try_register(&attacker, &name, &addr, &1);
        assert_eq!(result, Err(Ok(RegistryError::Unauthorized)));
    }

    #[test]
    fn test_transfer_admin() {
        let (env, admin, client) = setup();
        let new_admin = Address::generate(&env);
        client.transfer_admin(&admin, &new_admin);
        assert_eq!(client.admin(), new_admin);
    }

    #[test]
    fn test_register_emits_event() {
        let (env, admin, client) = setup();
        let name = String::from_str(&env, "oracle");
        let addr = Address::generate(&env);
        client.register(&admin, &name, &addr, &1);

        let event = env.events().all().last().unwrap().clone();
        assert_eq!(event.0, client.address);
        assert_eq!(
            event.1,
            vec![
                &env,
                Symbol::new(&env, "contract_registered").into_val(&env)
            ]
        );
        let (n, v, r): (String, u32, Option<String>) = event.2.into_val(&env);
        assert_eq!(n, name);
        assert_eq!(v, 1u32);
        assert_eq!(r, None);
    }

    #[test]
    fn test_deprecate_emits_event() {
        let (env, admin, client) = setup();
        let name = String::from_str(&env, "oracle");
        let addr = Address::generate(&env);
        client.register(&admin, &name, &addr, &1);
        client.deprecate(&admin, &name, &1, &None);
        client.deprecate(&admin, &name, &1, &None::<String>);

        let event = env.events().all().last().unwrap().clone();
        assert_eq!(event.0, client.address);
        assert_eq!(
            event.1,
            vec![
                &env,
                Symbol::new(&env, "contract_deprecated").into_val(&env)
            ]
        );
        let (n, v, r): (String, u32, Option<String>) = event.2.into_val(&env);
        assert_eq!(n, name);
        assert_eq!(v, 1u32);
        assert_eq!(r, None);
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
        let (old, new): (Address, Address) = event.2.into_val(&env);
        assert_eq!(old, admin);
        assert_eq!(new, new_admin);
    }

    #[test]
    fn test_register_higher_after_deprecation_succeeds() {
        let (env, admin, client) = setup();
        let name = String::from_str(&env, "oracle");
        let addr = Address::generate(&env);
        client.register(&admin, &name, &addr, &1);
        client.deprecate(&admin, &name, &1, &None);
        client.deprecate(&admin, &name, &1, &None::<String>);

        // Registering a higher version after deprecation should succeed
        client.register(&admin, &name, &addr, &2);
        let latest = client.get_latest(&name);
        assert_eq!(latest.version, 2);
    }

    #[test]
    fn test_get_latest_all_deprecated_fails() {
        let (env, admin, client) = setup();
        let name = String::from_str(&env, "oracle");
        let addr = Address::generate(&env);
        client.register(&admin, &name, &addr, &1);
        client.register(&admin, &name, &addr, &2);
        client.deprecate(&admin, &name, &1, &None);
        client.deprecate(&admin, &name, &2, &None);
        client.deprecate(&admin, &name, &1, &None::<String>);
        client.deprecate(&admin, &name, &2, &None::<String>);

        // When all versions are deprecated, get_latest should return NotFound
        let result = client.try_get_latest(&name);
        assert_eq!(result, Err(Ok(RegistryError::AllVersionsDeprecated)));
    }

    #[test]
    fn test_get_latest_unknown_name_fails() {
        let (env, _admin, client) = setup();
        let name = String::from_str(&env, "nonexistent");
        let result = client.try_get_latest(&name);
        assert_eq!(result, Err(Ok(RegistryError::NotFound)));
    }

    #[test]
    fn test_versions_unknown_name_returns_empty() {
        let (env, _admin, client) = setup();
        let name = String::from_str(&env, "nonexistent");
        let versions = client.versions(&name);
        assert!(versions.is_empty());
    }

    #[test]
    fn test_deprecate_many_all_succeed() {
        let (env, admin, client) = setup();
        let name = String::from_str(&env, "oracle");
        let (a1, a2, a3) = (
            Address::generate(&env),
            Address::generate(&env),
            Address::generate(&env),
        );
        client.register(&admin, &name, &a1, &1);
        client.register(&admin, &name, &a2, &2);
        client.register(&admin, &name, &a3, &3);

        let entries = vec![
            &env,
            (name.clone(), 1u32),
            (name.clone(), 2u32),
            (name.clone(), 3u32),
        ];
        let results = client.deprecate_many(&admin, &entries);
        assert_eq!(results.len(), 3);
        for r in results.iter() {
            assert_eq!(r, Ok(()));
        }
    }

    #[test]
    fn test_deprecate_many_partial_errors() {
        let (env, admin, client) = setup();
        let name = String::from_str(&env, "oracle");
        let addr = Address::generate(&env);
        client.register(&admin, &name, &addr, &1);

        let entries = vec![
            &env,
            (name.clone(), 1u32),  // ok
            (name.clone(), 99u32), // VersionNotFound
            (name.clone(), 1u32),  // AlreadyDeprecated
        ];
        let results = client.deprecate_many(&admin, &entries);
        assert_eq!(results.get(0).unwrap(), Ok(()));
        assert_eq!(results.get(1).unwrap(), Err(RegistryError::VersionNotFound));
        assert_eq!(
            results.get(2).unwrap(),
            Err(RegistryError::AlreadyDeprecated)
        );
    }

    #[test]
    fn test_deprecate_many_unauthorized_caller() {
        let (env, admin, client) = setup();
        let unauthorized = Address::generate(&env);
        let name = String::from_str(&env, "oracle");
        let (a1, a2, a3) = (
            Address::generate(&env),
            Address::generate(&env),
            Address::generate(&env),
        );
        
        // Register some versions as admin
        client.register(&admin, &name, &a1, &1);
        client.register(&admin, &name, &a2, &2);
        client.register(&admin, &name, &a3, &3);

        // Try to deprecate with unauthorized caller
        let entries = vec![
            &env,
            (name.clone(), 1u32),
            (name.clone(), 2u32),
            (name.clone(), 3u32),
        ];
        
        let result = client.try_deprecate_many(&unauthorized, &entries);
        assert_eq!(result, Err(Ok(RegistryError::Unauthorized)));
    }

    #[test]
    fn test_get_latest_with_constraint_exact_match() {
        let (env, admin, client) = setup();
        let name = String::from_str(&env, "oracle");
        let (a1, a2, a3) = (
            Address::generate(&env),
            Address::generate(&env),
            Address::generate(&env),
        );
        client.register(&admin, &name, &a1, &1);
        client.register(&admin, &name, &a2, &2);
        client.register(&admin, &name, &a3, &3);

        let constraint = String::from_str(&env, "2");
        let result = client.try_get_latest_with_constraint(&name, &Some(constraint));
        assert!(result.is_ok());
        let entry = result.unwrap().unwrap();
        assert_eq!(entry.version, 2);
    }

    #[test]
    fn test_get_latest_with_constraint_gte() {
        let (env, admin, client) = setup();
        let name = String::from_str(&env, "oracle");
        let (a1, a2, a3) = (
            Address::generate(&env),
            Address::generate(&env),
            Address::generate(&env),
        );
        client.register(&admin, &name, &a1, &1);
        client.register(&admin, &name, &a2, &2);
        client.register(&admin, &name, &a3, &3);

        let constraint = String::from_str(&env, ">=2");
        let result = client.try_get_latest_with_constraint(&name, &Some(constraint));
        assert!(result.is_ok());
        let entry = result.unwrap().unwrap();
        assert_eq!(entry.version, 3);
    }

    #[test]
    fn test_get_latest_with_constraint_lt() {
        let (env, admin, client) = setup();
        let name = String::from_str(&env, "oracle");
        let (a1, a2, a3) = (
            Address::generate(&env),
            Address::generate(&env),
            Address::generate(&env),
        );
        client.register(&admin, &name, &a1, &1);
        client.register(&admin, &name, &a2, &2);
        client.register(&admin, &name, &a3, &3);

        let constraint = String::from_str(&env, "<3");
        let result = client.try_get_latest_with_constraint(&name, &Some(constraint));
        assert!(result.is_ok());
        let entry = result.unwrap().unwrap();
        assert_eq!(entry.version, 2);
    }

    #[test]
    fn test_get_latest_with_constraint_caret() {
        let (env, admin, client) = setup();
        let name = String::from_str(&env, "oracle");
        let (a1, a2, a3) = (
            Address::generate(&env),
            Address::generate(&env),
            Address::generate(&env),
        );
        client.register(&admin, &name, &a1, &1);
        client.register(&admin, &name, &a2, &2);
        client.register(&admin, &name, &a3, &3);

        let constraint = String::from_str(&env, "^2");
        let result = client.try_get_latest_with_constraint(&name, &Some(constraint));
        assert!(result.is_ok());
        let entry = result.unwrap().unwrap();
        assert_eq!(entry.version, 2);
    }

    #[test]
    fn test_get_latest_with_constraint_no_match() {
        let (env, admin, client) = setup();
        let name = String::from_str(&env, "oracle");
        let (a1, a2) = (Address::generate(&env), Address::generate(&env));
        client.register(&admin, &name, &a1, &1);
        client.register(&admin, &name, &a2, &2);

        let constraint = String::from_str(&env, ">=5");
        let result = client.try_get_latest_with_constraint(&name, &Some(constraint));
        assert_eq!(result, Err(Ok(RegistryError::NotFound)));
    }

    #[test]
    fn test_get_latest_with_constraint_skips_deprecated() {
        let (env, admin, client) = setup();
        let name = String::from_str(&env, "oracle");
        let (a1, a2, a3) = (
            Address::generate(&env),
            Address::generate(&env),
            Address::generate(&env),
        );
        client.register(&admin, &name, &a1, &1);
        client.register(&admin, &name, &a2, &2);
        client.register(&admin, &name, &a3, &3);
        client.deprecate(&admin, &name, &3, &None);
        client.deprecate(&admin, &name, &3, &None::<String>);

        let constraint = String::from_str(&env, ">=2");
        let result = client.try_get_latest_with_constraint(&name, &Some(constraint));
        assert!(result.is_ok());
        let entry = result.unwrap().unwrap();
        assert_eq!(entry.version, 2);
    }

    #[test]
    fn test_get_latest_with_constraint_all_deprecated_returns_all_versions_deprecated() {
        let (env, admin, client) = setup();
        let name = String::from_str(&env, "oracle");
        let (a1, a2, a3) = (
            Address::generate(&env),
            Address::generate(&env),
            Address::generate(&env),
        );
        client.register(&admin, &name, &a1, &1);
        client.register(&admin, &name, &a2, &2);
        client.register(&admin, &name, &a3, &3);
        client.deprecate(&admin, &name, &1, &None);
        client.deprecate(&admin, &name, &2, &None);
        client.deprecate(&admin, &name, &3, &None);
        client.deprecate(&admin, &name, &1, &None::<String>);
        client.deprecate(&admin, &name, &2, &None::<String>);
        client.deprecate(&admin, &name, &3, &None::<String>);
        let constraint = String::from_str(&env, ">=1");
        let result = client.try_get_latest_with_constraint(&name, &Some(constraint));
        assert_eq!(result, Err(Ok(RegistryError::AllVersionsDeprecated)));
    }

    #[test]
    fn test_get_latest_with_constraint_empty_registry() {
        let (env, _admin, client) = setup();
        let name = String::from_str(&env, "oracle");
        let constraint = String::from_str(&env, ">=1");
        let result = client.try_get_latest_with_constraint(&name, &Some(constraint));
        assert_eq!(result, Err(Ok(RegistryError::NotFound)));
    }

    #[test]
    fn test_get_latest_with_constraint_invalid_constraint() {
        let (env, admin, client) = setup();
        let name = String::from_str(&env, "oracle");
        let addr = Address::generate(&env);
        client.register(&admin, &name, &addr, &1);

        let bad_constraint = String::from_str(&env, "abc");
        let result = client.try_get_latest_with_constraint(&name, &Some(bad_constraint));
        assert_eq!(result, Err(Ok(RegistryError::InvalidConstraint)));
    }

    #[test]
    fn test_get_latest_with_constraint_none_behaves_like_get_latest() {
        // Verifies the no-constraint path returns the same result as get_latest
        let (env, admin, client) = setup();
        let name = String::from_str(&env, "oracle");
        let addr = Address::generate(&env);
        client.register(&admin, &name, &addr, &1);
        let latest = client.get_latest(&name);
        let constrained = client.get_latest_with_constraint(&name, &None);
        assert_eq!(latest.version, constrained.version);
        assert_eq!(latest.address, constrained.address);
    }

    #[test]
    fn test_constraint_all_deprecated_returns_all_deprecated() {
        let (env, admin, client) = setup();
        let name = String::from_str(&env, "oracle");
        let addr = Address::generate(&env);
        client.register(&admin, &name, &addr, &1);
        client.register(&admin, &name, &addr, &2);
        client.register(&admin, &name, &addr, &3);
        client.deprecate(&admin, &name, &1, &None);
        client.deprecate(&admin, &name, &2, &None);
        client.deprecate(&admin, &name, &3, &None);
        client.deprecate(&admin, &name, &1, &None::<String>);
        client.deprecate(&admin, &name, &2, &None::<String>);
        client.deprecate(&admin, &name, &3, &None::<String>);

        let constraint = String::from_str(&env, ">=1");
        let result = client.try_get_latest_with_constraint(&name, &Some(constraint));
        assert_eq!(result, Err(Ok(RegistryError::AllVersionsDeprecated)));
    }

    #[test]
    fn test_constraint_no_matching_version_returns_not_found() {
        let (env, admin, client) = setup();
        let name = String::from_str(&env, "oracle");
        let (a1, a2) = (Address::generate(&env), Address::generate(&env));
        client.register(&admin, &name, &a1, &1);
        client.register(&admin, &name, &a2, &2);

        let constraint = String::from_str(&env, ">=5");
        let result = client.try_get_latest_with_constraint(&name, &Some(constraint));
        assert_eq!(result, Err(Ok(RegistryError::NotFound)));
    }

    #[test]
    fn test_get_all_names_empty() {
        let (env, _admin, client) = setup();
        let names = client.get_all_names();
        assert!(names.is_empty());
    }

    #[test]
    fn test_get_all_names_multiple() {
        let (env, admin, client) = setup();
        let name1 = String::from_str(&env, "oracle");
        let name2 = String::from_str(&env, "vault");
        let addr1 = Address::generate(&env);
        let addr2 = Address::generate(&env);

        client.register(&admin, &name1, &addr1, &1);
        client.register(&admin, &name2, &addr2, &1);

        let names = client.get_all_names();
        assert_eq!(names.len(), 2);
        assert!(names.contains(&name1));
        assert!(names.contains(&name2));
    }

    #[test]
    fn test_get_all_names_no_duplicates() {
        let (env, admin, client) = setup();
        let name = String::from_str(&env, "oracle");
        let addr1 = Address::generate(&env);
        let addr2 = Address::generate(&env);

        client.register(&admin, &name, &addr1, &1);
        client.register(&admin, &name, &addr2, &2);

        let names = client.get_all_names();
        assert_eq!(names.len(), 1);
        assert!(names.contains(&name));
    }

    #[test]
    fn test_get_all_versions_includes_deprecated() {
        let (env, admin, client) = setup();
        let name = String::from_str(&env, "oracle");
        let a1 = Address::generate(&env);
        let a2 = Address::generate(&env);
        client.register(&admin, &name, &a1, &1);
        client.register(&admin, &name, &a2, &2);
        client.deprecate(&admin, &name, &1, &None::<String>);
        client.deprecate(&admin, &name, &1, &None);

        let entries = client.get_all_versions(&name);
        assert_eq!(entries.len(), 2);
        let v1 = entries.iter().find(|e| e.version == 1).unwrap();
        let v2 = entries.iter().find(|e| e.version == 2).unwrap();
        assert!(v1.deprecated);
        assert!(!v2.deprecated);
    }

    #[test]
    fn test_get_all_versions_empty_for_unknown_name() {
        let (env, _admin, client) = setup();
        let name = String::from_str(&env, "unknown");
        assert!(client.get_all_versions(&name).is_empty());
    }

    #[test]
    fn test_deprecate_all_versions() {
        let (env, admin, client) = setup();
        let name = String::from_str(&env, "oracle");
        let (a1, a2, a3) = (
            Address::generate(&env),
            Address::generate(&env),
            Address::generate(&env),
        );
        client.register(&admin, &name, &a1, &1);
        client.register(&admin, &name, &a2, &2);
        client.register(&admin, &name, &a3, &3);

        client.deprecate_all_versions(&admin, &name, &None::<String>);

        let result = client.try_get_latest(&name);
        assert_eq!(result, Err(Ok(RegistryError::AllVersionsDeprecated)));
        // All entries should be marked deprecated
        for v in [1u32, 2, 3] {
            assert!(client.get(&name, &v).deprecated);
        }
    }

    #[test]
    fn test_deprecate_all_versions_skips_already_deprecated() {
    fn test_deprecate_with_reason_stored_and_emitted() {
        let (env, admin, client) = setup();
        let name = String::from_str(&env, "oracle");
        let addr = Address::generate(&env);
        let reason = String::from_str(&env, "security vulnerability in v1");
        client.register(&admin, &name, &addr, &1);
        client.deprecate(&admin, &name, &1, &Some(reason.clone()));

        // Verify reason is stored in the entry
        let entry = client.get(&name, &1);
        assert!(entry.deprecated);
        assert_eq!(entry.deprecation_reason, Some(reason.clone()));

        // Verify reason is emitted in the event
        let event = env.events().all().last().unwrap().clone();
        let (n, v, r): (String, u32, Option<String>) = event.2.into_val(&env);
        assert_eq!(n, name);
        assert_eq!(v, 1u32);
        assert_eq!(r, Some(reason));
    fn test_get_entry_by_address_found() {
    fn test_get_latest_with_constraint_version_zero_matching() {
        // version 0 is not allowed to be registered, but constraint >=0 should match version 1
        let (env, admin, client) = setup();
        let name = String::from_str(&env, "oracle");
        let addr = Address::generate(&env);
        client.register(&admin, &name, &addr, &1);
        // Pre-deprecate v1
        client.deprecate(&admin, &name, &1, &None::<String>);
        // deprecate_all_versions must not error on already-deprecated entries
        assert!(client
            .try_deprecate_all_versions(&admin, &name, &None::<String>)
            .is_ok());
    }

    #[test]
    fn test_deprecate_all_versions_unauthorized_fails() {
        let (env, _admin, client) = setup();
        let name = String::from_str(&env, "oracle");
        let attacker = Address::generate(&env);
        let result = client.try_deprecate_all_versions(&attacker, &name, &None::<String>);
        assert_eq!(result, Err(Ok(RegistryError::Unauthorized)));
    }

    #[test]
    fn test_get_entry_by_address_found() {
        let entry = client.get_entry_by_address(&addr).unwrap();
        assert_eq!(entry.address, addr);
        assert_eq!(entry.name, name);
        assert_eq!(entry.version, 1);
    }

    #[test]
    fn test_get_entry_by_address_not_found() {
        let (env, _admin, client) = setup();
        let unknown = Address::generate(&env);
        assert!(client.get_entry_by_address(&unknown).is_none());
    }

    #[test]
    fn test_deprecate_stores_reason() {

        let constraint = String::from_str(&env, ">=0");
         let result = client.get_latest_with_constraint(&name, &Some(constraint));
         assert_eq!(result.version, 1);
    }

    #[test]
    fn test_get_latest_with_constraint_large_version() {
        let (env, admin, client) = setup();
        let name = String::from_str(&env, "oracle");
        let addr = Address::generate(&env);
        let large_v = u32::MAX;
        client.register(&admin, &name, &addr, &large_v);

        let constraint = String::from_str(&env, "4294967295"); // u32::MAX
        let result = client.get_latest_with_constraint(&name, &Some(constraint));
        assert_eq!(result.version, large_v);

        let gte_constraint = String::from_str(&env, ">=4294967295");
        let result_gte = client.get_latest_with_constraint(&name, &Some(gte_constraint));
        assert_eq!(result_gte.version, large_v);
    }

    #[test]
    fn test_get_latest_with_constraint_caret_zero() {
        let (env, admin, client) = setup();
        let name = String::from_str(&env, "oracle");
        let addr = Address::generate(&env);
        client.register(&admin, &name, &addr, &1);
        let entry = client.get_entry_by_address(&addr);
        assert!(entry.is_some());
        let e = entry.unwrap();
        assert_eq!(e.address, addr);
        assert_eq!(e.version, 1);
    }

    #[test]
    fn test_get_entry_by_address_not_found() {
        let (env, _admin, client) = setup();
        let unknown = Address::generate(&env);
        assert!(client.get_entry_by_address(&unknown).is_none());
    }

    #[test]
    fn test_get_entry_by_address_none_after_deprecate() {
        let reason = String::from_str(&env, "security vulnerability");
        client.deprecate(&admin, &name, &1, &Some(reason.clone()));
        let entry = client.get(&name, &1);
        assert!(entry.deprecated);
        assert_eq!(entry.deprecation_reason, Some(reason));
    }

    #[test]
    fn test_deprecate_without_reason() {

        // ^0 means >=0 and <1. Since version 1 is registered, ^0 should NOT match.
        let constraint = String::from_str(&env, "^0");
        let result = client.try_get_latest_with_constraint(&name, &Some(constraint));
        assert_eq!(result, Err(Ok(RegistryError::NotFound)));
    }

    #[test]
    fn test_get_latest_with_constraint_tilde_zero() {
        let (env, admin, client) = setup();
        let name = String::from_str(&env, "oracle");
        let addr = Address::generate(&env);
        client.register(&admin, &name, &addr, &1);
        client.deprecate(&admin, &name, &1, &None::<String>);
        // Index is removed on deprecation
        assert!(client.get_entry_by_address(&addr).is_none());
        client.deprecate(&admin, &name, &1, &None);
        let entry = client.get(&name, &1);
        assert!(entry.deprecated);
        assert_eq!(entry.deprecation_reason, None);

        // ~0 means >=0 and <1. Since version 1 is registered, ~0 should NOT match.
        let constraint = String::from_str(&env, "~0");
        let result = client.try_get_latest_with_constraint(&name, &Some(constraint));
        assert_eq!(result, Err(Ok(RegistryError::NotFound)));
    }

    #[test]
    fn test_get_latest_with_constraint_invalid_strings() {
        let (env, admin, client) = setup();
        let name = String::from_str(&env, "oracle");
        let addr = Address::generate(&env);
        client.register(&admin, &name, &addr, &1);

        let invalid_cases = vec![
            &env,
            String::from_str(&env, ""),
            String::from_str(&env, ">="),
            String::from_str(&env, "^"),
            String::from_str(&env, "~"),
            String::from_str(&env, "v1"),
            String::from_str(&env, "1.0.0"), // we only support u32
        ];

        for constraint in invalid_cases.iter() {
            let result = client.try_get_latest_with_constraint(&name, &Some(constraint));
            assert_eq!(result, Err(Ok(RegistryError::InvalidConstraint)));
        }
    }
}

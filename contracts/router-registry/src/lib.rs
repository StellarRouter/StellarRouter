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

extern crate alloc;
use alloc::string::ToString;
use soroban_sdk::{
    contract, contracterror, contractimpl, contracttype, Address, Env, String, Symbol, Vec,
};

// ── Storage Keys ──────────────────────────────────────────────────────────────

#[contracttype]
pub enum DataKey {
    Admin,
    Entry(String, u32), // (name, version) -> ContractEntry
    Versions(String),   // name -> Vec<u32>
    ContractNames,      // Vec<String> of all registered names
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
        Self::require_admin(&env, &caller)?;

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
            registered_by: caller,
        };

        env.storage()
            .instance()
            .set(&DataKey::Entry(name.clone(), version), &entry);

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

        env.events()
            .publish((Symbol::new(&env, "contract_registered"),), (name, version));

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
        let versions = Self::get_versions_list(&env, &name);
        if versions.is_empty() {
            return Err(RegistryError::NotFound);
        }
        // Iterate in reverse to find latest non-deprecated
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
            if !entry.deprecated {
                return Ok(entry);
            }
        }
        Err(RegistryError::AllVersionsDeprecated)
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

        // If no constraint, use get_latest logic
        if constraint.is_none() {
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
                if !entry.deprecated {
                    return Ok(entry);
                }
            }
            return Err(RegistryError::AllVersionsDeprecated);
        }

        let constraint_str = constraint.unwrap();

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
                any_constraint_match = true;
                if !entry.deprecated {
                    return Ok(entry);
                }
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
    ) -> Result<(), RegistryError> {
        caller.require_auth();
        Self::require_admin(&env, &caller)?;
        Self::deprecate_one(&env, name, version)
    }

    fn deprecate_one(env: &Env, name: String, version: u32) -> Result<(), RegistryError> {
        let mut entry: ContractEntry = env
            .storage()
            .instance()
            .get(&DataKey::Entry(name.clone(), version))
            .ok_or(RegistryError::VersionNotFound)?;

        if entry.deprecated {
            return Err(RegistryError::AlreadyDeprecated);
        }

        entry.deprecated = true;
        env.storage()
            .instance()
            .set(&DataKey::Entry(name.clone(), version), &entry);
        env.events()
            .publish((Symbol::new(&env, "contract_deprecated"),), (name, version));
        Ok(())
    }

    pub fn deprecate_many(
        env: Env,
        caller: Address,
        entries: Vec<(String, u32)>,
    ) -> Vec<Result<(), RegistryError>> {
        caller.require_auth();
        if Self::require_admin(&env, &caller).is_err() {
            let mut results = Vec::new(&env);
            for _ in entries.iter() {
                results.push_back(Err(RegistryError::Unauthorized));
            }
            return results;
        }
        let mut results = Vec::new(&env);
        for (name, version) in entries.iter() {
            results.push_back(Self::deprecate_one(&env, name, version));
        }
        results
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
        Self::require_admin(&env, &current)?;
        env.storage().instance().set(&DataKey::Admin, &new_admin);
        env.events().publish(
            (Symbol::new(&env, "admin_transferred"),),
            (current, new_admin),
        );
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
    /// Note: This is a breaking change from the previous Result-based API.
    /// Calling admin() on an uninitialized contract is considered a programming error
    /// rather than a runtime condition, consistent with how similar getters work.
    pub fn admin(env: Env) -> Address {
        env.storage()
            .instance()
            .get(&DataKey::Admin)
            .expect("not initialized")
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

    /// Find the registry entry for a given contract address.
    ///
    /// Returns the first entry found (search order: all names in registration order,
    /// all versions in ascending order). Returns None if the address is not registered.
    ///
    /// # Arguments
    /// * `env` - The Soroban environment.
    /// * `address` - The contract address to look up.
    ///
    /// # Returns
    /// An [`Option<ContractEntry>`] containing the entry if found, None otherwise.
    pub fn get_entry_by_address(env: Env, address: Address) -> Option<ContractEntry> {
        let names: Vec<String> = env.storage()
            .instance()
            .get(&DataKey::ContractNames)
            .unwrap_or_else(|| Vec::new(&env));
        
        for name in names.iter() {
            let versions = Self::get_versions_list(&env, &name);
            for v in versions.iter() {
                if let Some(entry) = env.storage()
                    .instance()
                    .get::<DataKey, ContractEntry>(&DataKey::Entry(name.clone(), v))
                {
                    if entry.address == address {
                        return Some(entry);
                    }
                }
            }
        }
        None
    }

    // ── Helpers ───────────────────────────────────────────────────────────────

    fn require_admin(env: &Env, caller: &Address) -> Result<(), RegistryError> {
        let admin = Self::admin(env.clone());
        if &admin != caller {
            return Err(RegistryError::Unauthorized);
        }
        Ok(())
    }

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
        client.deprecate(&admin, &name, &2);
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
        let result = client.try_deprecate(&admin, &name, &99);
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
        client.deprecate(&admin, &name, &1);
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
        client.deprecate(&admin, &name, &3);
        client.deprecate(&admin, &name, &2);
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
        let (n, v): (String, u32) = event.2.into_val(&env);
        assert_eq!(n, name);
        assert_eq!(v, 1u32);
    }

    #[test]
    fn test_deprecate_emits_event() {
        let (env, admin, client) = setup();
        let name = String::from_str(&env, "oracle");
        let addr = Address::generate(&env);
        client.register(&admin, &name, &addr, &1);
        client.deprecate(&admin, &name, &1);

        let event = env.events().all().last().unwrap().clone();
        assert_eq!(event.0, client.address);
        assert_eq!(
            event.1,
            vec![
                &env,
                Symbol::new(&env, "contract_deprecated").into_val(&env)
            ]
        );
        let (n, v): (String, u32) = event.2.into_val(&env);
        assert_eq!(n, name);
        assert_eq!(v, 1u32);
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
        client.deprecate(&admin, &name, &1);

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
        client.deprecate(&admin, &name, &1);
        client.deprecate(&admin, &name, &2);

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
        client.deprecate(&admin, &name, &3);

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
        client.deprecate(&admin, &name, &1);
        client.deprecate(&admin, &name, &2);
        client.deprecate(&admin, &name, &3);

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
        client.deprecate(&admin, &name, &1);
        client.deprecate(&admin, &name, &2);
        client.deprecate(&admin, &name, &3);

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
}

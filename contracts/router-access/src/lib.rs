#![no_std]

//! # router-access
//!
//! Role-based access control for the stellar-router suite.
//! Supports arbitrary roles, multi-admin, per-address whitelisting,
//! and a role hierarchy where parent roles implicitly include child roles.
//!
//! ## Role Hierarchy
//!
//! Roles can be arranged in a parent → child relationship. Granting a parent
//! role to an address implicitly grants all of its child roles (transitively).
//!
//! ## Events (following naming convention: past tense verbs in snake_case)
//! - `role_granted` — Role granted to address (account, role, expiry_timestamp)
//! - `role_revoked` — Role revoked from address (role, target)
//! - `role_parent_set` — Parent role set (role, parent_role)
//! - `role_parent_removed` — Parent role removed (role)
//! - `role_admin_set` — Admin set for role (role, admin)
//! - `address_blacklisted` — Address blacklisted (address)
//! - `address_unblacklisted` — Address unblacklisted (address)
//! - `role_expired` — Role grant expired (role, target)
//! - `admin_transferred` — Admin transferred (old_admin, new_admin)
//!
//! ## Storage model
//!
//! - `HasRole(role, address)` — explicit direct grant
//! - `RoleParent(role)` — the single parent role of `role` (if any)
//! - `RoleAdmin(role)` — address allowed to grant/revoke `role`

use soroban_sdk::{
    contract, contracterror, contractimpl, contracttype, Address, Env, String, Symbol, Vec,
};

// ── Storage Keys ──────────────────────────────────────────────────────────────

#[contracttype]
pub enum DataKey {
    SuperAdmin,
    HasRole(String, Address),
    RoleAdmin(String),
    Blacklisted(Address),
    RoleParent(String),
    RoleMembers(String),
    RoleMember(String, u32),
    RoleMemberIndex(String, Address),
    RoleMemberCount(String),
    AddressRoles(Address),
    RoleExpiry(String, Address),
    BlacklistReason(Address),
    BlacklistExpiry(Address),
}

// ── Errors ────────────────────────────────────────────────────────────────────

#[contracterror]
#[derive(Copy, Clone, Debug, PartialEq)]
pub enum AccessError {
    AlreadyInitialized = 1,
    NotInitialized = 2,
    Unauthorized = 3,
    AlreadyHasRole = 4,
    RoleNotFound = 5,
    Blacklisted = 6,
    CannotBlacklistAdmin = 7,
    HierarchyCycle = 8,
}

// ── Contract ──────────────────────────────────────────────────────────────────

#[contract]
pub struct RouterAccess;

/// Maximum depth to walk when resolving inherited roles.
const MAX_HIERARCHY_DEPTH: u32 = 16;

#[contractimpl]
impl RouterAccess {
    /// Initialize with a super-admin.
    pub fn initialize(env: Env, super_admin: Address) -> Result<(), AccessError> {
        if env.storage().instance().has(&DataKey::SuperAdmin) {
            return Err(AccessError::AlreadyInitialized);
        }
        env.storage()
            .instance()
            .set(&DataKey::SuperAdmin, &super_admin);
        Ok(())
    }

    /// Grant a role to an address. Caller must be super-admin or role admin.
    pub fn grant_role(
        env: Env,
        admin: Address,
        account: Address,
        role: String,
        expires_in: Option<u64>,
    ) -> Result<(), AccessError> {
        admin.require_auth();
        Self::require_role_manager(&env, &admin, &role)?;
        Self::grant_role_internal(&env, &account, &role, expires_in)
    }

    /// Grant a role to multiple targets in one call, returning per-target results.
    ///
    /// Caller must be super-admin or role admin. Auth is checked once upfront.
    /// Each target is processed independently so partial failures are captured.
    pub fn bulk_grant_role(
        env: Env,
        caller: Address,
        role: String,
        targets: Vec<Address>,
        expires_in: Option<u64>,
    ) -> Result<Vec<Result<(), AccessError>>, AccessError> {
        caller.require_auth();
        Self::require_role_manager(&env, &caller, &role)?;
        let mut results = Vec::new(&env);
        for target in targets.iter() {
            results.push_back(Self::grant_role_internal(&env, &target, &role, expires_in));
        }
        Ok(results)
    }

    /// Grant a role to multiple accounts in one call (accounts-first variant).
    pub fn grant_role_batch(
        env: Env,
        admin: Address,
        accounts: Vec<Address>,
        role: String,
        expires_in: Option<u64>,
    ) -> Result<Vec<Result<(), AccessError>>, AccessError> {
        admin.require_auth();
        Self::require_role_manager(&env, &admin, &role)?;
        let mut results = Vec::new(&env);
        for account in accounts.iter() {
            results.push_back(Self::grant_role_internal(&env, &account, &role, expires_in));
        }
        env.storage()
            .instance()
            .set(&DataKey::HasRole(role.clone(), target.clone()), &true);

        env.events().publish(
            (Symbol::new(&env, router_common::EVENT_ROLE_GRANTED),),
            (role, target),
            .set(&DataKey::AddressRoles(account.clone()), &roles);

        // Set expiry timestamp
        let key = DataKey::RoleExpiry(role.clone(), account.clone());
        env.storage().instance().set(&key, &expiry_timestamp);

        env.events().publish(
            (Symbol::new(&env, router_common::EVENT_ROLE_GRANTED),),
            (account, role, expiry_timestamp),
        );
        Ok(())
    }

    /// Revoke a direct role grant from an address.
    pub fn revoke_role(
        env: Env,
        caller: Address,
        role: String,
        target: Address,
    ) -> Result<(), AccessError> {
        caller.require_auth();
        Self::require_role_manager(&env, &caller, &role)?;
        Self::revoke_role_internal(&env, &role, &target)
    }

    /// Revoke a role from multiple accounts in one call.
    pub fn revoke_role_batch(
        env: Env,
        caller: Address,
        role: String,
        targets: Vec<Address>,
    ) -> Result<Vec<Result<(), AccessError>>, AccessError> {
        caller.require_auth();
        Self::require_role_manager(&env, &caller, &role)?;
        let mut results = Vec::new(&env);
        for target in targets.iter() {
            results.push_back(Self::revoke_role_internal(&env, &role, &target));
        }

        env.storage().instance().remove(&key);
        env.storage()
            .instance()
            .remove(&DataKey::RoleExpiry(role.clone(), target.clone()));
        env.storage()
            .instance()
            .remove(&DataKey::RoleMemberIndex(role.clone(), target.clone()));

        env.events().publish(
            (Symbol::new(&env, router_common::EVENT_ROLE_REVOKED),),
            (role, target),
        );
        Ok(())
    }

    /// Check if an address has a role — directly or via the hierarchy.
    pub fn has_role(env: Env, role: String, target: Address) -> bool {
        if Self::is_blacklisted_internal(&env, &target) {
            return false;
        }
        Self::has_role_internal(&env, &role, &target)
    }

    /// Set the parent role for a role (defines the hierarchy edge).
    pub fn set_role_parent(
        env: Env,
        caller: Address,
        role: String,
        parent_role: String,
    ) -> Result<(), AccessError> {
        caller.require_auth();
        Self::require_super_admin(&env, &caller)?;
        if Self::is_ancestor(&env, &parent_role, &role) {
            return Err(AccessError::HierarchyCycle);
        }
        env.storage()
            .instance()
            .set(&DataKey::RoleParent(role.clone()), &parent_role);

        env.events().publish(
            (Symbol::new(&env, router_common::EVENT_ROLE_PARENT_SET),),
            (role, parent_role),
        );
        Ok(())
    }

    /// Remove the parent relationship for a role.
    pub fn remove_role_parent(env: Env, caller: Address, role: String) -> Result<(), AccessError> {
        caller.require_auth();
        Self::require_super_admin(&env, &caller)?;
        env.storage().instance().remove(&DataKey::RoleParent(role.clone()));
        env.events().publish(
            (Symbol::new(&env, router_common::EVENT_ROLE_PARENT_REMOVED),),
            role,
        );
        Ok(())
    }

    /// Get the direct parent role of a role, if one is set.
    pub fn get_role_parent(env: Env, role: String) -> Option<String> {
        env.storage()
            .instance()
            .remove(&DataKey::RoleParent(role.clone()));
        env.events()
            .publish((Symbol::new(&env, "role_parent_removed"),), role);
        Ok(())
    }

    /// Get the direct parent role of a role, if one is set.
    pub fn get_role_parent(env: Env, role: String) -> Option<String> {
        env.storage().instance().get(&DataKey::RoleParent(role))
    }

    /// Set the admin for a specific role (who can grant/revoke it).
    pub fn set_role_admin(
        env: Env,
        caller: Address,
        role: String,
        admin: Address,
    ) -> Result<(), AccessError> {
        caller.require_auth();
        Self::require_super_admin(&env, &caller)?;
        if Self::is_blacklisted_internal(&env, &admin) {
            return Err(AccessError::Blacklisted);
        }
        env.storage()
            .instance()
            .set(&DataKey::RoleAdmin(role.clone()), &admin);
        env.events()
            .publish((Symbol::new(&env, "role_admin_set"),), (role, admin));
        Ok(())
    }

    /// Returns the role admin for the given role, or None if none is set.
    pub fn get_role_admin(env: Env, role: String) -> Option<Address> {
        env.storage()
            .instance()
            .get::<DataKey, Address>(&DataKey::RoleAdmin(role))
    }

    /// Returns `true` if `addr` is the designated admin for `role`.
    ///
    /// Convenience wrapper around [`Self::get_role_admin`] that avoids
    /// callers having to unwrap an `Option` and compare addresses themselves.
    pub fn is_role_admin(env: Env, role: String, addr: Address) -> bool {
        env.storage()
            .instance()
            .get::<DataKey, Address>(&DataKey::RoleAdmin(role))
            .map(|admin| admin == addr)
            .unwrap_or(false)
    }

    /// Blacklist an address.
    pub fn blacklist(
        env: Env,
        caller: Address,
        target: Address,
        reason: Option<String>,
        expires_at: Option<u64>,
    ) -> Result<(), AccessError> {
        caller.require_auth();
        Self::require_super_admin(&env, &caller)?;
        let super_admin: Address = env
            .storage()
            .instance()
            .get(&DataKey::SuperAdmin)
            .ok_or(AccessError::NotInitialized)?;
        if target == super_admin {
            return Err(AccessError::CannotBlacklistAdmin);
        }
        env.storage()
            .instance()
            .set(&DataKey::Blacklisted(target.clone()), &true);
        if let Some(r) = reason.clone() {
            env.storage()
                .instance()
                .set(&DataKey::BlacklistReason(target.clone()), &r);
        }
        if let Some(exp) = expires_at {
            env.storage()
                .instance()
                .set(&DataKey::BlacklistExpiry(target.clone()), &exp);
        }
        env.events().publish(
            (Symbol::new(&env, router_common::EVENT_ADDRESS_BLACKLISTED),),
            (target, reason, expires_at),
        );
        Ok(())
    }

    /// Remove an address from the blacklist.
    pub fn unblacklist(env: Env, caller: Address, target: Address) -> Result<(), AccessError> {
        caller.require_auth();
        Self::require_super_admin(&env, &caller)?;
        env.storage()
            .instance()
            .remove(&DataKey::Blacklisted(target.clone()));
        env.storage()
            .instance()
            .remove(&DataKey::BlacklistReason(target.clone()));
        env.storage()
            .instance()
            .remove(&DataKey::BlacklistExpiry(target.clone()));
        env.events()
            .publish((Symbol::new(&env, "address_unblacklisted"),), target);
        Ok(())
    }

    pub fn is_blacklisted(env: Env, target: Address) -> bool {
        Self::is_blacklisted_internal(&env, &target)
    }

    /// Check if a role has expired for an address.
    pub fn is_role_expired(env: Env, role: String, target: Address) -> bool {
        if let Some(expires_at) = env
            .storage()
            .instance()
            .get::<DataKey, u64>(&DataKey::RoleExpiry(role, target))
        {
            env.ledger().timestamp() >= expires_at
        } else {
            false
        }
    }

    pub fn get_roles_for_address(env: Env, addr: Address) -> Vec<String> {
        env.storage()
            .instance()
            .get(&DataKey::AddressRoles(addr))
            .unwrap_or_else(|| Vec::new(&env))
    }

    pub fn transfer_super_admin(
        env: Env,
        current: Address,
        new_admin: Address,
    ) -> Result<(), AccessError> {
        current.require_auth();
        Self::require_super_admin(&env, &current)?;
        env.storage().instance().set(&DataKey::SuperAdmin, &new_admin);
        env.storage()
            .instance()
            .set(&DataKey::SuperAdmin, &new_admin);
        env.events().publish(
            (Symbol::new(&env, router_common::EVENT_ADMIN_TRANSFERRED),),
            (current, new_admin),
        );
        Ok(())
    }

    /// Get current super-admin.
    ///
    /// # Errors
    /// * [`AccessError::NotInitialized`] — contract not initialized.
    pub fn super_admin(env: Env) -> Result<Address, AccessError> {
        env.storage()
            .instance()
            .get::<DataKey, u64>(&DataKey::RoleExpiry(role, target))
    }

    /// Force-expire a role grant (super-admin only).
    pub fn expire_role(
        env: Env,
        caller: Address,
        role: String,
        target: Address,
    ) -> Result<(), AccessError> {
        caller.require_auth();
        Self::require_super_admin(&env, &caller)?;
        env.storage()
            .instance()
            .remove(&DataKey::RoleExpiry(role.clone(), target.clone()));
        env.storage()
            .instance()
            .remove(&DataKey::HasRole(role.clone(), target.clone()));
        env.storage()
            .instance()
            .remove(&DataKey::RoleMemberIndex(role.clone(), target.clone()));
        env.events()
            .publish((Symbol::new(&env, "role_expired"),), (role, target));
        Ok(())
    }

    /// Get paginated active members of a role.
    pub fn get_role_members(env: Env, role: String, offset: u32, limit: u32) -> Vec<Address> {
        if limit == 0 {
            return Vec::new(&env);
        }
        let total: u32 = env
            .storage()
            .instance()
            .get(&DataKey::RoleMemberCount(role.clone()))
            .unwrap_or(0);
        if offset >= total {
            return Vec::new(&env);
        }
        let end = core::cmp::min(total, offset.saturating_add(limit));
        let mut active_members = Vec::new(&env);
        for i in offset..end {
            if let Some(member) = env
                .storage()
                .instance()
                .get::<DataKey, Address>(&DataKey::RoleMember(role.clone(), i))
            {
                if Self::has_role_internal(&env, &role, &member) {
                    active_members.push_back(member);
                }
            }
        }
        active_members
    }

    /// Revoke a role from multiple accounts in one call.
    ///
    /// Calls [`Self::revoke_role`] for each target and returns `Ok(())` only
    /// if all revocations succeed.
    ///
    /// # Errors
    /// * [`AccessError::Unauthorized`] — caller is not super-admin or role admin.
    /// * [`AccessError::RoleNotFound`] — a target does not hold the role directly.
    pub fn bulk_revoke_role(
        env: Env,
        caller: Address,
        role: String,
        targets: Vec<Address>,
    ) -> Result<(), AccessError> {
        caller.require_auth();
        for target in targets.iter() {
            Self::revoke_role(env.clone(), caller.clone(), role.clone(), target.clone())?;
        }
        Ok(())
    }

    /// Revoke a role from multiple accounts in one call.
    ///
    /// Returns a vector of per-account results so partial failures are visible.
    ///
    /// # Errors
    /// * [`AccessError::Unauthorized`] — caller is not super-admin or role admin.
    pub fn revoke_role_batch(
        env: Env,
        current: Address,
        new_admin: Address,
    ) -> Result<(), AccessError> {
        current.require_auth();
        Self::require_super_admin(&env, &current)?;
        env.storage()
            .instance()
            .set(&DataKey::SuperAdmin, &new_admin);
        env.events().publish(
            (Symbol::new(&env, "admin_transferred"),),
            (current, new_admin),
        );
        Ok(())
    }

    /// Get current super-admin.
    pub fn super_admin(env: Env) -> Result<Address, AccessError> {
        env.storage()
            .instance()
            .get(&DataKey::SuperAdmin)
            .ok_or(AccessError::NotInitialized)
    }

    // ── Helpers ───────────────────────────────────────────────────────────────

    fn require_super_admin(env: &Env, caller: &Address) -> Result<(), AccessError> {
        let admin: Address = env
            .storage()
            .instance()
            .get(&DataKey::SuperAdmin)
            .ok_or(AccessError::NotInitialized)?;
        if &admin != caller {
            return Err(AccessError::Unauthorized);
        }
        Ok(())
    }

    fn require_role_manager(env: &Env, caller: &Address, role: &String) -> Result<(), AccessError> {
        if Self::is_blacklisted_internal(env, caller) {
            return Err(AccessError::Blacklisted);
        }
        if let Some(admin) = env
            .storage()
            .instance()
            .get::<DataKey, Address>(&DataKey::SuperAdmin)
        {
            if &admin == caller {
                return Ok(());
            }
        }
        if let Some(role_admin) = env
            .storage()
            .instance()
            .get::<DataKey, Address>(&DataKey::RoleAdmin(role.clone()))
        {
            if &role_admin == caller {
                return Ok(());
            }
        }
        Err(AccessError::Unauthorized)
    }

    /// Returns true if `target` holds `role` directly OR via the hierarchy.
    fn has_role_internal(env: &Env, role: &String, target: &Address) -> bool {
        let mut current = role.clone();
        let mut depth = 0u32;
        loop {
            // Check direct grant at this level (and expiry)
            let has = env
                .storage()
                .instance()
                .get::<DataKey, bool>(&DataKey::HasRole(current.clone(), target.clone()))
                .unwrap_or(false);
            if has {
                // Check expiry
                let expired = env
                    .storage()
                    .instance()
                    .get::<DataKey, u64>(&DataKey::RoleExpiry(current.clone(), target.clone()))
                    .map(|exp| env.ledger().timestamp() >= exp)
                    .unwrap_or(false);
                if !expired {
                    return true;
                }
            }
            // Walk up to parent
            match env
                .storage()
                .instance()
                .get::<DataKey, String>(&DataKey::RoleParent(current))
            {
                Some(parent) => {
                    depth += 1;
                    if depth >= MAX_HIERARCHY_DEPTH {
                        return false;
                    }
                    current = parent;
                }
                None => return false,
            }
        }
    }

    /// Returns true if `ancestor` is an ancestor of `role` in the hierarchy.
    fn is_ancestor(env: &Env, role: &String, ancestor: &String) -> bool {
        let mut current = role.clone();
        let mut depth = 0u32;
        loop {
            if &current == ancestor {
                return true;
            }
            match env
                .storage()
                .instance()
                .get::<DataKey, String>(&DataKey::RoleParent(current))
            {
                Some(parent) => {
                    depth += 1;
                    if depth >= MAX_HIERARCHY_DEPTH {
                        return false;
                    }
                    current = parent;
                }
                None => return false,
            }
        }
    }

    fn is_blacklisted_internal(env: &Env, target: &Address) -> bool {
        let is_blacklisted = env
            .storage()
            .instance()
            .get::<DataKey, bool>(&DataKey::Blacklisted(target.clone()))
            .unwrap_or(false);
        if !is_blacklisted {
            return false;
        }
        // If an expiry is set and has passed, treat as not blacklisted
        if let Some(expires_at) = env
            .storage()
            .instance()
            .get::<DataKey, u64>(&DataKey::BlacklistExpiry(target.clone()))
        {
            if env.ledger().timestamp() >= expires_at {
                env.storage()
                    .instance()
                    .remove(&DataKey::Blacklisted(target.clone()));
                env.storage()
                    .instance()
                    .remove(&DataKey::BlacklistExpiry(target.clone()));
                env.storage()
                    .instance()
                    .remove(&DataKey::BlacklistReason(target.clone()));
                return false;
            }
        }
        true
    }

    fn grant_role_internal(
        env: &Env,
        account: &Address,
        role: &String,
        expires_in: Option<u64>,
    ) -> Result<(), AccessError> {
        if Self::is_blacklisted_internal(env, account) {
            return Err(AccessError::Blacklisted);
        }
        if Self::has_role_internal(env, role, account) {
            return Err(AccessError::AlreadyHasRole);
        }
        let expiry_timestamp = match expires_in {
            Some(seconds) => env.ledger().timestamp() + seconds,
            None => u64::MAX,
        };
        env.storage()
            .instance()
            .set(&DataKey::HasRole(role.clone(), account.clone()), &true);
        // Indexed member storage
        if !env
            .storage()
            .instance()
            .has(&DataKey::RoleMemberIndex(role.clone(), account.clone()))
        {
            let count: u32 = env
                .storage()
                .instance()
                .get(&DataKey::RoleMemberCount(role.clone()))
                .unwrap_or(0);
            env.storage()
                .instance()
                .set(&DataKey::RoleMember(role.clone(), count), account);
            env.storage().instance().set(
                &DataKey::RoleMemberIndex(role.clone(), account.clone()),
                &count,
            );
            env.storage()
                .instance()
                .set(&DataKey::RoleMemberCount(role.clone()), &(count + 1));
        }
        // AddressRoles list
        let mut roles: Vec<String> = env
            .storage()
            .instance()
            .get(&DataKey::AddressRoles(account.clone()))
            .unwrap_or_else(|| Vec::new(env));
        if !roles.iter().any(|r| r == *role) {
            roles.push_back(role.clone());
        }
        env.storage()
            .instance()
            .set(&DataKey::AddressRoles(account.clone()), &roles);
        env.storage().instance().set(
            &DataKey::RoleExpiry(role.clone(), account.clone()),
            &expiry_timestamp,
        );
        env.events().publish(
            (Symbol::new(env, "role_granted"),),
            (account.clone(), role.clone(), expiry_timestamp),
        );
        Ok(())
    }

    fn revoke_role_internal(env: &Env, role: &String, target: &Address) -> Result<(), AccessError> {
        let key = DataKey::HasRole(role.clone(), target.clone());
        if !env.storage().instance().has(&key) {
            return Err(AccessError::RoleNotFound);
        }
        env.storage().instance().remove(&key);
        env.storage()
            .instance()
            .remove(&DataKey::RoleExpiry(role.clone(), target.clone()));
        env.storage()
            .instance()
            .remove(&DataKey::RoleMemberIndex(role.clone(), target.clone()));
        let mut roles: Vec<String> = env
            .storage()
            .instance()
            .get(&DataKey::AddressRoles(target.clone()))
            .unwrap_or_else(|| Vec::new(env));
        if let Some(i) = roles.iter().position(|r| r == *role) {
            roles.remove(i as u32);
        }
        env.storage()
            .instance()
            .set(&DataKey::AddressRoles(target.clone()), &roles);
        env.events().publish(
            (Symbol::new(env, "role_revoked"),),
            (role.clone(), target.clone()),
        );
        Ok(())
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    extern crate std;
    use super::*;
    use soroban_sdk::{
        testutils::{Address as _, Events, Ledger},
        Env, IntoVal, Symbol,
    };

    fn setup() -> (Env, Address, RouterAccessClient<'static>) {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register_contract(None, RouterAccess);
        let client = RouterAccessClient::new(&env, &contract_id);
        let admin = Address::generate(&env);
        client.initialize(&admin);
        (env, admin, client)
    }

    #[test]
    fn test_expired_role_not_recognized() {
        let (env, admin, client) = setup();
        let role = String::from_str(&env, "operator");
        let user = Address::generate(&env);
        client.grant_role(&admin, &user, &role, &Some(10));
        env.ledger().set_timestamp(env.ledger().timestamp() + 20);
        assert!(!client.has_role(&role, &user));
    }

    #[test]
    fn test_role_expires_correctly_with_timestamp() {
        let (env, admin, client) = setup();
        let role = String::from_str(&env, "operator");
        let user = Address::generate(&env);
        client.grant_role(&admin, &user, &role, &Some(1));
        env.ledger().set_timestamp(env.ledger().timestamp() + 5);
        assert!(!client.has_role(&role, &user));
    }

    #[test]
    fn test_set_role_admin_emits_event() {
        let (env, admin, client) = setup();
        let role = String::from_str(&env, "operator");
        let new_role_admin = Address::generate(&env);
        client.set_role_admin(&admin, &role, &new_role_admin);
        let events = env.events().all();
        let last = events.last().unwrap();
        let topic: Symbol = last.1.get(0).unwrap().into_val(&env);
        assert_eq!(topic, Symbol::new(&env, "role_admin_set"));
        let (emitted_role, emitted_admin): (String, Address) = last.2.into_val(&env);
        assert_eq!(emitted_role, role);
        assert_eq!(emitted_admin, new_role_admin);
    }

    #[test]
    fn test_set_role_admin_rejects_blacklisted_address() {
        let (env, admin, client) = setup();
        let role = String::from_str(&env, "operator");
        let blacklisted_addr = Address::generate(&env);
        client.blacklist(&admin, &blacklisted_addr, &None::<String>, &None);
        let result = client.try_set_role_admin(&admin, &role, &blacklisted_addr);
        assert_eq!(result, Err(Ok(AccessError::Blacklisted)));
    }

    #[test]
    fn test_set_role_admin_valid_address_succeeds() {
        let (env, admin, client) = setup();
        let role = String::from_str(&env, "operator");
        let valid_addr = Address::generate(&env);
        client.set_role_admin(&admin, &role, &valid_addr);
        let events = env.events().all();
        let last = events.last().unwrap();
        let topic: Symbol = last.1.get(0).unwrap().into_val(&env);
        assert_eq!(topic, Symbol::new(&env, "role_admin_set"));
        let (emitted_role, emitted_admin): (String, Address) = last.2.into_val(&env);
        assert_eq!(emitted_role, role);
        assert_eq!(emitted_admin, valid_addr);
    }

    #[test]
    fn test_blacklisted_role_admin_cannot_grant() {
        let (env, admin, client) = setup();
        let role = String::from_str(&env, "editor");
        let attacker = Address::generate(&env);
        let victim = Address::generate(&env);
        client.set_role_admin(&admin, &role, &attacker);
        client.blacklist(&admin, &attacker, &None::<String>, &None);
        let result = client.try_grant_role(&attacker, &victim, &role, &None);
        assert_eq!(result, Err(Ok(AccessError::Blacklisted)));
    }

    #[test]
    fn test_blacklisted_role_admin_cannot_revoke() {
        let (env, admin, client) = setup();
        let role = String::from_str(&env, "editor");
        let attacker = Address::generate(&env);
        let victim = Address::generate(&env);
        client.set_role_admin(&admin, &role, &attacker);
        client.grant_role(&admin, &victim, &role, &None);
        client.blacklist(&admin, &attacker, &None::<String>, &None);
        let result = client.try_revoke_role(&attacker, &role, &victim);
        assert_eq!(result, Err(Ok(AccessError::Blacklisted)));
    }

    #[test]
    fn test_revoke_role_succeeds_after_grant() {
        let (env, admin, client) = setup();
        let role = String::from_str(&env, "editor");
        let user = Address::generate(&env);
        client.grant_role(&admin, &user, &role, &None);
        let result = client.try_revoke_role(&admin, &role, &user);
        assert!(result.is_ok(), "revoke_role should succeed after grant");
        assert!(!client.has_role(&role, &user));
    }

    #[test]
    fn test_revoke_role_removes_expiry() {
        let (env, admin, client) = setup();
        let role = String::from_str(&env, "editor");
        let user = Address::generate(&env);
        client.grant_role(&admin, &user, &role, &Some(100));
        client.revoke_role(&admin, &role, &user);
        assert!(!client.is_role_expired(&role, &user));
        let has_expiry: bool = env.as_contract(&client.address, || {
            env.storage()
                .instance()
                .has(&DataKey::RoleExpiry(role.clone(), user.clone()))
        });
        assert!(!has_expiry);
    }

    #[test]
    fn test_get_role_members_populated_after_grant() {
        let (env, admin, client) = setup();
        let role = String::from_str(&env, "editor");
        let user1 = Address::generate(&env);
        let user2 = Address::generate(&env);
        let members_before = client.get_role_members(&role, &0, &50);
        assert!(members_before.is_empty());
        client.grant_role(&admin, &user1, &role, &None);
        let members_after_first = client.get_role_members(&role, &0, &50);
        assert_eq!(members_after_first.len(), 1);
        assert!(members_after_first.contains(&user1));
        client.grant_role(&admin, &user2, &role, &None);
        let members_after_second = client.get_role_members(&role, &0, &50);
        assert_eq!(members_after_second.len(), 2);
        assert!(members_after_second.contains(&user1));
        assert!(members_after_second.contains(&user2));
    }

    #[test]
    fn test_grant_role_blacklisted_account_fails() {
        let (env, admin, client) = setup();
        let role = String::from_str(&env, "operator");
        let blacklisted_user = Address::generate(&env);
        client.blacklist(&admin, &blacklisted_user, &None::<String>, &None);
        let result = client.try_grant_role(&admin, &blacklisted_user, &role, &None);
        assert_eq!(result, Err(Ok(AccessError::Blacklisted)));
    }

    #[test]
    fn test_grant_role_already_has_role_fails() {
        let (env, admin, client) = setup();
        let role = String::from_str(&env, "operator");
        let user = Address::generate(&env);
        client.grant_role(&admin, &user, &role, &None);
        let result = client.try_grant_role(&admin, &user, &role, &None);
        assert_eq!(result, Err(Ok(AccessError::AlreadyHasRole)));
    }

    #[test]
    fn test_grant_role_returns_error_on_unauthorized() {
        let (env, _admin, client) = setup();
        let role = String::from_str(&env, "operator");
        let unauthorized = Address::generate(&env);
        let user = Address::generate(&env);
        let result = client.try_grant_role(&unauthorized, &user, &role, &None);
        assert_eq!(result, Err(Ok(AccessError::Unauthorized)));
    }

    #[test]
    fn test_blacklisted_address_cannot_use_role() {
        let (env, admin, client) = setup();
        let role = String::from_str(&env, "operator");
        let user = Address::generate(&env);
        client.grant_role(&admin, &user, &role, &None);
        assert!(client.has_role(&role, &user));
        client.blacklist(&admin, &user, &None::<String>, &None);
        assert!(!client.has_role(&role, &user));
        client.unblacklist(&admin, &user);
        assert!(client.has_role(&role, &user));
    }

    // ── Issue #443: blacklist expires_at ──────────────────────────────────────

    #[test]
    fn test_blacklist_expires_after_timestamp() {
        let (env, admin, client) = setup();
        let user = Address::generate(&env);

        // Blacklist with expiry 100 seconds from now (ledger timestamp starts at 0)
        client.blacklist(&admin, &user, &None::<String>, &Some(100u64));
        assert!(client.is_blacklisted(&user));

        // Advance time past expiry
        env.ledger().set_timestamp(101);
        // Expired blacklist should be treated as not blacklisted
        assert!(!client.is_blacklisted(&user));
    }

    #[test]
    fn test_blacklist_without_expiry_is_permanent() {
        let (env, admin, client) = setup();
        let user = Address::generate(&env);

        client.blacklist(&admin, &user, &None::<String>, &None);
        assert!(client.is_blacklisted(&user));

        // Advance time significantly — should still be blacklisted
        env.ledger().set_timestamp(999_999);
        assert!(client.is_blacklisted(&user));
    }

    #[test]
    fn test_expired_blacklist_allows_role_grant() {
        let (env, admin, client) = setup();
        let role = String::from_str(&env, "operator");
        let user = Address::generate(&env);

        // Blacklist with short expiry
        client.blacklist(&admin, &user, &None::<String>, &Some(50u64));
        assert!(client.is_blacklisted(&user));

        // Advance past expiry
        env.ledger().set_timestamp(51);
        assert!(!client.is_blacklisted(&user));

        // Should now be able to grant role
        assert!(client.try_grant_role(&admin, &user, &role, &None).is_ok());
    }

    #[test]
    fn test_get_roles_for_address_populated_after_grant() {
        let (env, admin, client) = setup();
        let user = Address::generate(&env);
        let role1 = String::from_str(&env, "editor");
        let role2 = String::from_str(&env, "viewer");
        let roles_before = client.get_roles_for_address(&user);
        assert!(roles_before.is_empty());
        client.grant_role(&admin, &user, &role1, &None);
        let roles_after_first = client.get_roles_for_address(&user);
        assert_eq!(roles_after_first.len(), 1);
        assert!(roles_after_first.contains(&role1));
        client.grant_role(&admin, &user, &role2, &None);
        let roles_after_second = client.get_roles_for_address(&user);
        assert_eq!(roles_after_second.len(), 2);
        assert!(roles_after_second.contains(&role1));
        assert!(roles_after_second.contains(&role2));
    }

    #[test]
    fn test_old_super_admin_locked_out_after_transfer() {
        let (env, admin, client) = setup();
        let new_admin = Address::generate(&env);
        client.transfer_super_admin(&admin, &new_admin);
        let role = String::from_str(&env, "operator");
        let user = Address::generate(&env);
        assert_eq!(
            client.try_grant_role(&admin, &user, &role, &None),
            Err(Ok(AccessError::Unauthorized))
        );
        assert!(client
            .try_grant_role(&new_admin, &user, &role, &None)
            .is_ok());
    }

    #[test]
    fn test_transfer_super_admin_to_self_succeeds() {
        let (env, admin, client) = setup();
        assert!(client.try_transfer_super_admin(&admin, &admin).is_ok());
        assert_eq!(client.super_admin(), admin);
    }

    #[test]
    fn test_transfer_super_admin_unauthorized_fails() {
        let (env, _admin, client) = setup();
        let attacker = Address::generate(&env);
        assert_eq!(
            client.try_transfer_super_admin(&attacker, &attacker),
            Err(Ok(AccessError::Unauthorized))
        );
    }

    #[test]
    fn test_revoke_role_removes_storage_key() {
        let (env, admin, client) = setup();
        let role = String::from_str(&env, "operator");
        let user = Address::generate(&env);
        client.grant_role(&admin, &user, &role, &None);
        assert!(client.has_role(&role, &user));
        client.revoke_role(&admin, &role, &user);
        assert!(!client.has_role(&role, &user));
        assert!(client.try_grant_role(&admin, &user, &role, &None).is_ok());
        assert!(client.has_role(&role, &user));
    }

    #[test]
    fn test_revoke_nonexistent_role_fails() {
        let (env, admin, client) = setup();
        let role = String::from_str(&env, "operator");
        let user = Address::generate(&env);
        let result = client.try_revoke_role(&admin, &role, &user);
        assert_eq!(result, Err(Ok(AccessError::RoleNotFound)));
    }

    #[test]
    fn test_expire_role_removes_access() {
        let (env, admin, client) = setup();
        let role = String::from_str(&env, "operator");
        let user = Address::generate(&env);
        client.grant_role(&admin, &user, &role, &Some(9999));
        assert!(client.has_role(&role, &user));
        client.expire_role(&admin, &role, &user);
        assert!(!client.has_role(&role, &user));
    }

    #[test]
    fn test_expire_role_allows_regrant() {
        let (env, admin, client) = setup();
        let role = String::from_str(&env, "operator");
        let user = Address::generate(&env);
        client.grant_role(&admin, &user, &role, &Some(9999));
        client.expire_role(&admin, &role, &user);
        assert!(client
            .try_grant_role(&admin, &user, &role, &Some(9999))
            .is_ok());
        assert!(client.has_role(&role, &user));
    }

    #[test]
    fn test_expire_role_unauthorized_fails() {
        let (env, admin, client) = setup();
        let role = String::from_str(&env, "operator");
        let user = Address::generate(&env);
        let attacker = Address::generate(&env);
        client.grant_role(&admin, &user, &role, &Some(9999));
        let result = client.try_expire_role(&attacker, &role, &user);
        assert_eq!(result, Err(Ok(AccessError::Unauthorized)));
    }

    #[test]
    fn test_revoke_role_emits_event() {
        let (env, admin, client) = setup();
        let role = String::from_str(&env, "operator");
        let user = Address::generate(&env);
        client.grant_role(&admin, &user, &role, &None);
        client.revoke_role(&admin, &role, &user);
        let events = env.events().all();
        let last = events.last().unwrap();
        let topic: Symbol = last.1.get(0).unwrap().into_val(&env);
        assert_eq!(topic, Symbol::new(&env, "role_revoked"));
        let (emitted_role, emitted_target): (String, Address) = last.2.into_val(&env);
        assert_eq!(emitted_role, role);
        assert_eq!(emitted_target, user);
    }

    #[test]
    fn test_get_role_members_excludes_expired_roles() {
        let (env, admin, client) = setup();
        let role = String::from_str(&env, "operator");
        let user = Address::generate(&env);
        client.grant_role(&admin, &user, &role, &Some(10));
        let members_before = client.get_role_members(&role, &0, &50);
        assert!(members_before.contains(&user));
        env.ledger().set_timestamp(env.ledger().timestamp() + 20);
        assert!(!client.has_role(&role, &user));
        let members_after = client.get_role_members(&role, &0, &50);
        assert!(!members_after.contains(&user));
        assert!(members_after.is_empty());
    }

    #[test]
    fn test_get_role_members_supports_offset_limit_pagination() {
        let (env, admin, client) = setup();
        let role = String::from_str(&env, "operator");
        let user1 = Address::generate(&env);
        let user2 = Address::generate(&env);
        let user3 = Address::generate(&env);
        client.grant_role(&admin, &user1, &role, &None);
        client.grant_role(&admin, &user2, &role, &None);
        client.grant_role(&admin, &user3, &role, &None);
        let page = client.get_role_members(&role, &1, &1);
        assert_eq!(page.len(), 1);
        assert!(page.contains(&user2));
    }

    #[test]
    fn test_get_role_admin_returns_address_after_set() {
        let (env, admin, client) = setup();
        let role = String::from_str(&env, "operator");
        let role_admin = Address::generate(&env);
        client.set_role_admin(&admin, &role, &role_admin);
        assert_eq!(client.get_role_admin(&role), Some(role_admin));
    }

    #[test]
    fn test_get_role_admin_returns_none_when_not_set() {
        let (env, _admin, client) = setup();
        let role = String::from_str(&env, "operator");
        assert_eq!(client.get_role_admin(&role), None);
    }

    #[test]
    fn test_is_role_admin_returns_true_for_designated_admin() {
        let (env, admin, client) = setup();
        let role = String::from_str(&env, "operator");
        let role_admin = Address::generate(&env);

        client.set_role_admin(&admin, &role, &role_admin);

        assert!(client.is_role_admin(&role, &role_admin));
    }

    #[test]
    fn test_is_role_admin_returns_false_for_non_admin() {
        let (env, admin, client) = setup();
        let role = String::from_str(&env, "operator");
        let role_admin = Address::generate(&env);
        let other = Address::generate(&env);

        client.set_role_admin(&admin, &role, &role_admin);

        assert!(!client.is_role_admin(&role, &other));
    }

    #[test]
    fn test_is_role_admin_returns_false_when_no_admin_set() {
        let (env, _admin, client) = setup();
        let role = String::from_str(&env, "operator");
        let addr = Address::generate(&env);

        assert!(!client.is_role_admin(&role, &addr));
    }

    #[test]
    fn test_set_role_admin_unauthorized_fails() {
        let (env, _admin, client) = setup();
        let role = String::from_str(&env, "operator");
        let attacker = Address::generate(&env);
        let target = Address::generate(&env);
        let result = client.try_set_role_admin(&attacker, &role, &target);
        assert_eq!(result, Err(Ok(AccessError::Unauthorized)));
    }

    #[test]
    fn test_get_role_expiry_returns_timestamp() {
        let (env, admin, client) = setup();
        let role = String::from_str(&env, "operator");
        let user = Address::generate(&env);
        let now = env.ledger().timestamp();
        client.grant_role(&admin, &user, &role, &Some(100));
        let expiry = client.get_role_expiry(&role, &user);
        assert_eq!(expiry, Some(now + 100));
    }

    #[test]
    fn test_get_role_expiry_none_when_not_granted() {
        let (env, _admin, client) = setup();
        let role = String::from_str(&env, "operator");
        let user = Address::generate(&env);
        assert_eq!(client.get_role_expiry(&role, &user), None);
    }

    #[test]
    fn test_get_role_expiry_max_when_no_expiry() {
        let (env, admin, client) = setup();
        let role = String::from_str(&env, "operator");
        let user = Address::generate(&env);
        client.grant_role(&admin, &user, &role, &None);
        assert_eq!(client.get_role_expiry(&role, &user), Some(u64::MAX));
    }

    #[test]
    fn test_parent_role_grants_child_access() {
        let (env, admin, client) = setup();
        let viewer = String::from_str(&env, "viewer");
        let editor = String::from_str(&env, "editor");
        let user = Address::generate(&env);
        client.set_role_parent(&admin, &viewer, &editor);
        client.grant_role(&admin, &user, &editor, &None);
        assert!(client.has_role(&editor, &user));
        assert!(client.has_role(&viewer, &user));
    }

    #[test]
    fn test_transitive_hierarchy() {
        let (env, admin, client) = setup();
        let viewer = String::from_str(&env, "viewer");
        let editor = String::from_str(&env, "editor");
        let admin_role = String::from_str(&env, "admin");
        let user = Address::generate(&env);
        client.set_role_parent(&admin, &editor, &admin_role);
        client.set_role_parent(&admin, &viewer, &editor);
        client.grant_role(&admin, &user, &admin_role, &None);
        assert!(client.has_role(&admin_role, &user));
        assert!(client.has_role(&editor, &user));
        assert!(client.has_role(&viewer, &user));
    }

    #[test]
    fn test_no_inheritance_without_parent() {
        let (env, admin, client) = setup();
        let viewer = String::from_str(&env, "viewer");
        let editor = String::from_str(&env, "editor");
        let user = Address::generate(&env);
        client.grant_role(&admin, &user, &editor, &None);
        assert!(client.has_role(&editor, &user));
        assert!(!client.has_role(&viewer, &user));
    }

    #[test]
    fn test_set_role_parent_cycle_fails() {
        let (env, admin, client) = setup();
        let a = String::from_str(&env, "a");
        let b = String::from_str(&env, "b");
        client.set_role_parent(&admin, &b, &a);
        let result = client.try_set_role_parent(&admin, &a, &b);
        assert_eq!(result, Err(Ok(AccessError::HierarchyCycle)));
    }

    #[test]
    fn test_self_cycle_fails() {
        let (env, admin, client) = setup();
        let role = String::from_str(&env, "admin");
        let result = client.try_set_role_parent(&admin, &role, &role);
        assert_eq!(result, Err(Ok(AccessError::HierarchyCycle)));
    }

    #[test]
    fn test_remove_role_parent_breaks_inheritance() {
        let (env, admin, client) = setup();
        let viewer = String::from_str(&env, "viewer");
        let editor = String::from_str(&env, "editor");
        let user = Address::generate(&env);
        client.set_role_parent(&admin, &viewer, &editor);
        client.grant_role(&admin, &user, &editor, &None);
        assert!(client.has_role(&viewer, &user));
        client.remove_role_parent(&admin, &viewer);
        assert!(!client.has_role(&viewer, &user));
        assert!(client.has_role(&editor, &user));
    }

    #[test]
    fn test_get_role_parent() {
        let (env, admin, client) = setup();
        let viewer = String::from_str(&env, "viewer");
        let editor = String::from_str(&env, "editor");
        assert_eq!(client.get_role_parent(&viewer), None);
        client.set_role_parent(&admin, &viewer, &editor);
        assert_eq!(client.get_role_parent(&viewer), Some(editor));
    }

    #[test]
    fn test_blacklisted_user_fails_has_role_even_with_hierarchy() {
        let (env, admin, client) = setup();
        let viewer = String::from_str(&env, "viewer");
        let editor = String::from_str(&env, "editor");
        let user = Address::generate(&env);
        client.set_role_parent(&admin, &viewer, &editor);
        client.grant_role(&admin, &user, &editor, &None);
        assert!(client.has_role(&viewer, &user));
        client.blacklist(&admin, &user, &None::<String>, &None);
        assert!(!client.has_role(&viewer, &user));
        assert!(!client.has_role(&editor, &user));
    }

    #[test]
    fn test_unauthorized_set_role_parent_fails() {
        let (env, _admin, client) = setup();
        let attacker = Address::generate(&env);
        let a = String::from_str(&env, "a");
        let b = String::from_str(&env, "b");
        let result = client.try_set_role_parent(&attacker, &a, &b);
        assert_eq!(result, Err(Ok(AccessError::Unauthorized)));
    }

    // ── grant_role_batch tests ────────────────────────────────────────────────

    #[test]
    fn test_grant_role_batch_all_succeed() {
        let (env, admin, client) = setup();
        let role = String::from_str(&env, "operator");
        let u1 = Address::generate(&env);
        let u2 = Address::generate(&env);
        let accounts = soroban_sdk::vec![&env, u1.clone(), u2.clone()];
        let results = client.grant_role_batch(&admin, &accounts, &role, &None);
        assert_eq!(results.len(), 2);
        for r in results.iter() {
            assert_eq!(r, Ok(()));
        }
        assert!(client.has_role(&role, &u1));
        assert!(client.has_role(&role, &u2));
    }

    #[test]
    fn test_grant_role_batch_partial_errors() {
        let (env, admin, client) = setup();
        let role = String::from_str(&env, "operator");
        let u1 = Address::generate(&env);
        client.grant_role(&admin, &u1, &role, &None);
        let u2 = Address::generate(&env);
        let accounts = soroban_sdk::vec![&env, u1.clone(), u2.clone()];
        let results = client.grant_role_batch(&admin, &accounts, &role, &None);
        assert_eq!(results.get(0).unwrap(), Err(AccessError::AlreadyHasRole));
        assert_eq!(results.get(1).unwrap(), Ok(()));
    }

    #[test]
    fn test_grant_role_batch_unauthorized_fails() {
        let (env, _admin, client) = setup();
        let role = String::from_str(&env, "operator");
        let attacker = Address::generate(&env);
        let u1 = Address::generate(&env);
        let accounts = soroban_sdk::vec![&env, u1];
        let result = client.try_grant_role_batch(&attacker, &accounts, &role, &None);
        assert_eq!(result, Err(Ok(AccessError::Unauthorized)));
    }

    // ── revoke_role_batch tests ───────────────────────────────────────────────

    #[test]
    fn test_revoke_role_batch_all_succeed() {
        let (env, admin, client) = setup();
        let role = String::from_str(&env, "operator");
        let u1 = Address::generate(&env);
        let u2 = Address::generate(&env);
        client.grant_role(&admin, &u1, &role, &None);
        client.grant_role(&admin, &u2, &role, &None);
        let targets = soroban_sdk::vec![&env, u1.clone(), u2.clone()];
        let results = client.revoke_role_batch(&admin, &role, &targets);
        assert_eq!(results.len(), 2);
        for r in results.iter() {
            assert_eq!(r, Ok(()));
        }
        assert!(!client.has_role(&role, &u1));
        assert!(!client.has_role(&role, &u2));
    }

    #[test]
    fn test_revoke_role_batch_partial_errors() {
        let (env, admin, client) = setup();
        let role = String::from_str(&env, "operator");
        let u1 = Address::generate(&env);
        let u2 = Address::generate(&env);
        client.grant_role(&admin, &u1, &role, &None);
        let targets = soroban_sdk::vec![&env, u1.clone(), u2.clone()];
        let results = client.revoke_role_batch(&admin, &role, &targets);
        assert_eq!(results.get(0).unwrap(), Ok(()));
        assert_eq!(results.get(1).unwrap(), Err(AccessError::RoleNotFound));
    }

    #[test]
    fn test_revoke_role_batch_unauthorized_fails() {
        let (env, _admin, client) = setup();
        let role = String::from_str(&env, "operator");
        let attacker = Address::generate(&env);
        let u1 = Address::generate(&env);
        let targets = soroban_sdk::vec![&env, u1];
        let result = client.try_revoke_role_batch(&attacker, &role, &targets);
        assert_eq!(result, Err(Ok(AccessError::Unauthorized)));
    }

    // ── bulk_grant_role tests ─────────────────────────────────────────────────

    #[test]
    fn test_bulk_grant_role_all_succeed() {
        let (env, admin, client) = setup();
        let role = String::from_str(&env, "operator");
        let u1 = Address::generate(&env);
        let u2 = Address::generate(&env);
        let targets = soroban_sdk::vec![&env, u1.clone(), u2.clone()];
        let results = client.bulk_grant_role(&admin, &role, &targets, &None);
        assert_eq!(results.len(), 2);
        for r in results.iter() {
            assert_eq!(r, Ok(()));
        }
        assert!(client.has_role(&role, &u1));
        assert!(client.has_role(&role, &u2));
    }

    #[test]
    fn test_bulk_grant_role_partial_errors() {
        let (env, admin, client) = setup();
        let role = String::from_str(&env, "operator");
        let u1 = Address::generate(&env);
        client.grant_role(&admin, &u1, &role, &None);
        let u2 = Address::generate(&env);
        let targets = soroban_sdk::vec![&env, u1.clone(), u2.clone()];
        let results = client.bulk_grant_role(&admin, &role, &targets, &None);
        assert_eq!(results.get(0).unwrap(), Err(AccessError::AlreadyHasRole));
        assert_eq!(results.get(1).unwrap(), Ok(()));
        assert!(client.has_role(&role, &u2));
    }

    #[test]
    fn test_bulk_grant_role_blacklisted_target_fails() {
        let (env, admin, client) = setup();
        let role = String::from_str(&env, "operator");
        let u1 = Address::generate(&env);
        let u2 = Address::generate(&env);
        client.blacklist(&admin, &u1, &None::<String>, &None);
        let targets = soroban_sdk::vec![&env, u1.clone(), u2.clone()];
        let results = client.bulk_grant_role(&admin, &role, &targets, &None);
        assert_eq!(results.get(0).unwrap(), Err(AccessError::Blacklisted));
        assert_eq!(results.get(1).unwrap(), Ok(()));
    }

    #[test]
    fn test_bulk_grant_role_unauthorized_fails() {
        let (env, _admin, client) = setup();
        let role = String::from_str(&env, "operator");
        let attacker = Address::generate(&env);
        let u1 = Address::generate(&env);
        let targets = soroban_sdk::vec![&env, u1];
        let result = client.try_bulk_grant_role(&attacker, &role, &targets, &None);
        assert_eq!(result, Err(Ok(AccessError::Unauthorized)));
    }

    #[test]
    fn test_bulk_grant_role_with_expiry() {
        let (env, admin, client) = setup();
        let role = String::from_str(&env, "operator");
        let u1 = Address::generate(&env);
        let now = env.ledger().timestamp();
        let targets = soroban_sdk::vec![&env, u1.clone()];
        client.bulk_grant_role(&admin, &role, &targets, &Some(50));
        assert_eq!(client.get_role_expiry(&role, &u1), Some(now + 50));
        env.ledger().set_timestamp(now + 100);
        assert!(!client.has_role(&role, &u1));
    }

    #[test]
    fn test_bulk_grant_role_empty_targets() {
        let (env, admin, client) = setup();
        let role = String::from_str(&env, "operator");
        let targets: Vec<Address> = soroban_sdk::vec![&env];
        let results = client.bulk_grant_role(&admin, &role, &targets, &None);
        assert_eq!(results.len(), 0);
    }
}

    // ── Issue #510: Additional bulk operation tests ──────────────────────────

    #[test]
    fn test_bulk_grant_role_with_blacklisted_address() {
        let (env, admin, client) = setup();
        let role = String::from_str(&env, "operator");
        let u1 = Address::generate(&env);
        let u2 = Address::generate(&env);
        let u3 = Address::generate(&env);

        // Blacklist u2
        client.blacklist(&admin, &u2, &None::<String>, &None);

        let accounts = soroban_sdk::vec![&env, u1.clone(), u2.clone(), u3.clone()];
        let results = client.grant_role_batch(&admin, &accounts, &role, &None);

        // u1 should succeed, u2 should fail with Blacklisted, u3 should succeed
        assert_eq!(results.get(0).unwrap(), Ok(()));
        assert_eq!(results.get(1).unwrap(), Err(AccessError::Blacklisted));
        assert_eq!(results.get(2).unwrap(), Ok(()));

        // Verify final state
        assert!(client.has_role(&u1, &role));
        assert!(!client.has_role(&u2, &role));
        assert!(client.has_role(&u3, &role));
    }

    #[test]
    fn test_bulk_grant_role_empty_list() {
        let (env, admin, client) = setup();
        let role = String::from_str(&env, "operator");
        let accounts = soroban_sdk::Vec::new(&env);
        let results = client.grant_role_batch(&admin, &accounts, &role, &None);
        assert_eq!(results.len(), 0);
    }

    #[test]
    fn test_bulk_revoke_role_empty_list() {
        let (env, admin, client) = setup();
        let role = String::from_str(&env, "operator");
        let targets = soroban_sdk::Vec::new(&env);
        let results = client.revoke_role_batch(&admin, &role, &targets);
        assert_eq!(results.len(), 0);
    }

    #[test]
    fn test_bulk_grant_role_all_blacklisted() {
        let (env, admin, client) = setup();
        let role = String::from_str(&env, "operator");
        let u1 = Address::generate(&env);
        let u2 = Address::generate(&env);

        // Blacklist both
        client.blacklist(&admin, &u1, &None::<String>, &None);
        client.blacklist(&admin, &u2, &None::<String>, &None);

        let accounts = soroban_sdk::vec![&env, u1.clone(), u2.clone()];
        let results = client.grant_role_batch(&admin, &accounts, &role, &None);

        // Both should fail
        assert_eq!(results.get(0).unwrap(), Err(AccessError::Blacklisted));
        assert_eq!(results.get(1).unwrap(), Err(AccessError::Blacklisted));
    }
}

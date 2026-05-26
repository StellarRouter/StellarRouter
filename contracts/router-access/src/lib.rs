#![no_std]

//! # router-access
//!
//! Role-based access control for the stellar-router suite.

use soroban_sdk::{
    contract, contracterror, contractimpl, contracttype, Address, Env, String, Symbol, Vec,
};

// ── Storage Keys ──────────────────────────────────────────────────────────────

#[contracttype]
pub enum DataKey {
    SuperAdmin,
    HasRole(String, Address), // (role, address) -> bool
    RoleAdmin(String),        // role -> Address who manages it
    Blacklisted(Address),
    RoleMembers(String),   // role -> Vec<Address>
    AddressRoles(Address), // address -> Vec<String>
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
}

// ── Contract ──────────────────────────────────────────────────────────────────

#[contract]
pub struct RouterAccess;

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

    /// Grant a role to an address.
    pub fn grant_role(
        env: Env,
        admin: Address,
        account: Address,
        role: String,
        expires_in: Option<u64>,
    ) -> Result<(), AccessError> {
        admin.require_auth();
        Self::require_role_manager(&env, &admin, &role)?;

        if Self::is_blacklisted_internal(&env, &account) {
            return Err(AccessError::Blacklisted);
        }
        if Self::has_role_internal(&env, &account, &role) {
            return Err(AccessError::AlreadyHasRole);
        }

        let expiry_timestamp = match expires_in {
            Some(seconds) => env.ledger().timestamp() + seconds,
            None => u64::MAX,
        };

        // Set HasRole flag
        env.storage()
            .instance()
            .set(&DataKey::HasRole(role.clone(), account.clone()), &true);

        // Add to RoleMembers list (if not already present)
        let mut members: Vec<Address> = env
            .storage()
            .instance()
            .get(&DataKey::RoleMembers(role.clone()))
            .unwrap_or_else(|| Vec::new(&env));
        if !members.iter().any(|a| a == account) {
            members.push_back(account.clone());
        }
        env.storage()
            .instance()
            .set(&DataKey::RoleMembers(role.clone()), &members);

        // Add to AddressRoles list (if not already present)
        let mut roles: Vec<String> = env
            .storage()
            .instance()
            .get(&DataKey::AddressRoles(account.clone()))
            .unwrap_or_else(|| Vec::new(&env));
        if !roles.iter().any(|r| r == role) {
            roles.push_back(role.clone());
        }
        env.storage()
            .instance()
            .set(&DataKey::AddressRoles(account.clone()), &roles);

        // Set expiry timestamp
        let key = DataKey::RoleExpiry(role.clone(), account.clone());
        env.storage().instance().set(&key, &expiry_timestamp);

        env.events().publish(
            (Symbol::new(&env, "role_grant"),),
            (account, role, expiry_timestamp),
        );
        Ok(())
    }

    /// Removes `role` from `target`.
    pub fn revoke_role(
        env: Env,
        caller: Address,
        role: String,
        target: Address,
    ) -> Result<(), AccessError> {
        caller.require_auth();
        Self::require_role_manager(&env, &caller, &role)?;

        // Check the raw storage key — not has_role_internal — so that expired
        // roles (where has_role_internal returns false) can still be revoked
        // to clean up storage.
        let key = DataKey::HasRole(role.clone(), target.clone());
        if !env.storage().instance().has(&key) {
            return Err(AccessError::RoleNotFound);
        }

        env.storage().instance().remove(&key);

        let mut members: Vec<Address> = env
            .storage()
            .instance()
            .get(&DataKey::RoleMembers(role.clone()))
            .unwrap_or_else(|| Vec::new(&env));
        if let Some(i) = members.iter().position(|a| a == target) {
            members.remove(i as u32);
        }
        env.storage()
            .instance()
            .set(&DataKey::RoleMembers(role.clone()), &members);

        let mut roles: Vec<String> = env
            .storage()
            .instance()
            .get(&DataKey::AddressRoles(target.clone()))
            .unwrap_or_else(|| Vec::new(&env));
        if let Some(i) = roles.iter().position(|r| r == role) {
            roles.remove(i as u32);
        }
        env.storage()
            .instance()
            .set(&DataKey::AddressRoles(target.clone()), &roles);

        env.storage()
            .instance()
            .remove(&DataKey::RoleExpiry(role.clone(), target.clone()));

        env.events()
            .publish((Symbol::new(&env, "role_revoked"),), (role, target));
        Ok(())
    }

    /// Check if an address has a role (and it has not expired).
    pub fn has_role(env: Env, account: Address, role: String) -> bool {
        Self::has_role_internal(&env, &account, &role)
    }

    /// Check if a role has expired for an address.
    pub fn is_role_expired(env: Env, role: String, target: Address) -> bool {
        if let Some(expires_at) = env
            .storage()
            .instance()
            .get::<DataKey, u64>(&DataKey::RoleExpiry(role, target))
        {
            let current_timestamp = env.ledger().timestamp();
            current_timestamp >= expires_at
        } else {
            false
        }
    }

    /// Set the admin for a specific role.
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

        // Default storage of a blacklist entry; reason and expiry are optional
        env.storage()
            .instance()
            .set(&DataKey::Blacklisted(target.clone()), &true);

        if let Some(r) = reason {
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
            (Symbol::new(&env, "address_blacklisted"),),
            (target, reason, expires_at),
        );

        Ok(())
    }

    /// Remove from blacklist.
    pub fn unblacklist(env: Env, caller: Address, target: Address) -> Result<(), AccessError> {
        caller.require_auth();
        Self::require_super_admin(&env, &caller)?;
        env.storage().instance().remove(&DataKey::Blacklisted(target.clone()));
        env.storage().instance().remove(&DataKey::BlacklistReason(target.clone()));
        env.storage().instance().remove(&DataKey::BlacklistExpiry(target.clone()));
        env.events()
            .publish((Symbol::new(&env, "address_unblacklisted"),), target);
        Ok(())
    }

    pub fn is_blacklisted(env: Env, target: Address) -> bool {
        Self::is_blacklisted_internal(&env, &target)
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

        // If an expiry is set and has passed, treat as not blacklisted and clean up
        if let Some(expires_at) = env
            .storage()
            .instance()
            .get::<DataKey, u64>(&DataKey::BlacklistExpiry(target.clone()))
        {
            let current_timestamp = env.ledger().timestamp();
            if current_timestamp >= expires_at {
                // Expired: remove stored blacklist data
                env.storage().instance().remove(&DataKey::Blacklisted(target.clone()));
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

    pub fn get_role_members(env: Env, role: String) -> Vec<Address> {
        let all_members: Vec<Address> = env
            .storage()
            .instance()
            .get(&DataKey::RoleMembers(role.clone()))
            .unwrap_or_else(|| Vec::new(&env));

        // Filter out expired roles
        let mut active_members = Vec::new(&env);
        for member in all_members.iter() {
            if Self::has_role_internal(&env, &member, &role) {
                active_members.push_back(member.clone());
            }
        }
        active_members
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
        env.storage()
            .instance()
            .set(&DataKey::SuperAdmin, &new_admin);
        env.events().publish(
            (Symbol::new(&env, "admin_transferred"),),
            (current, new_admin),
        );
        Ok(())
    }

    pub fn super_admin(env: Env) -> Result<Address, AccessError> {
        env.storage()
            .instance()
            .get(&DataKey::SuperAdmin)
            .ok_or(AccessError::NotInitialized)
    }

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
        env.events()
            .publish((Symbol::new(&env, "role_expired"),), (role, target));
        Ok(())
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

    fn has_role_internal(env: &Env, account: &Address, role: &String) -> bool {
        if Self::is_blacklisted_internal(env, account) {
            return false;
        }

        let has_role = env
            .storage()
            .instance()
            .get::<DataKey, bool>(&DataKey::HasRole(role.clone(), account.clone()))
            .unwrap_or(false);

        if !has_role {
            return false;
        }

        // Check if role has expired
        if let Some(expires_at) = env
            .storage()
            .instance()
            .get::<DataKey, u64>(&DataKey::RoleExpiry(role.clone(), account.clone()))
        {
            let current_timestamp = env.ledger().timestamp();
            if current_timestamp >= expires_at {
                return false;
            }
        }

        true
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

    // ... (all your existing tests remain unchanged) ...

    #[test]
    fn test_expired_role_not_recognized() {
        let (env, admin, client) = setup();
        let role = String::from_str(&env, "operator");
        let user = Address::generate(&env);

        client.grant_role(&admin, &user, &role, &Some(10));

        env.ledger().set_timestamp(env.ledger().timestamp() + 20);

        assert!(!client.has_role(&user, &role));
    }

    #[test]
    fn test_role_expires_correctly_with_timestamp() {
        let (env, admin, client) = setup();
        let role = String::from_str(&env, "operator");
        let user = Address::generate(&env);

        client.grant_role(&admin, &user, &role, &Some(1));

        env.ledger().set_timestamp(env.ledger().timestamp() + 5);

        assert!(!client.has_role(&user, &role));
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

        // Blacklist the address
        client.blacklist(&admin, &blacklisted_addr, &None::<String>, &None);

        // Try to set blacklisted address as role admin
        let result = client.try_set_role_admin(&admin, &role, &blacklisted_addr);
        assert_eq!(result, Err(Ok(AccessError::Blacklisted)));
    }

    #[test]
    fn test_set_role_admin_valid_address_succeeds() {
        let (env, admin, client) = setup();
        let role = String::from_str(&env, "operator");
        let valid_addr = Address::generate(&env);

        // Set a non-blacklisted address as role admin
        client.set_role_admin(&admin, &role, &valid_addr);

        // Verify the role admin was set correctly
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

        // Designate attacker as editor admin
        client.set_role_admin(&admin, &role, &attacker);

        // Blacklist the attacker
        client.blacklist(&admin, &attacker, &None::<String>, &None);

        // Try to grant role - should fail with Blacklisted
        let result = client.try_grant_role(&attacker, &victim, &role, &None);
        assert_eq!(result, Err(Ok(AccessError::Blacklisted)));
    }

    #[test]
    fn test_blacklisted_role_admin_cannot_revoke() {
        let (env, admin, client) = setup();
        let role = String::from_str(&env, "editor");
        let attacker = Address::generate(&env);
        let victim = Address::generate(&env);

        // Designate attacker as editor admin
        client.set_role_admin(&admin, &role, &attacker);

        // Grant role to victim
        client
            .grant_role(&admin, &victim, &role, &None);

        // Blacklist the attacker
        client.blacklist(&admin, &attacker, &None::<String>, &None);

        // Try to revoke role - should fail with Blacklisted
        let result = client.try_revoke_role(&attacker, &role, &victim);
        assert_eq!(result, Err(Ok(AccessError::Blacklisted)));
    }

    // ── Issue #174: grant_role missing writes ────────────────────────────────

    #[test]
    fn test_revoke_role_succeeds_after_grant() {
        let (env, admin, client) = setup();
        let role = String::from_str(&env, "editor");
        let user = Address::generate(&env);

        // Grant the role
        client
            .grant_role(&admin, &user, &role, &None);

        // Revoke should succeed (not return RoleNotFound)
        let result = client.try_revoke_role(&admin, &role, &user);
        assert!(result.is_ok(), "revoke_role should succeed after grant");

        // Verify role is no longer present
        assert!(!client.has_role(&user, &role));
    }

    #[test]
    fn test_revoke_role_removes_expiry() {
        let (env, admin, client) = setup();
        let role = String::from_str(&env, "editor");
        let user = Address::generate(&env);

        client
            .grant_role(&admin, &user, &role, &Some(100));

        client.revoke_role(&admin, &role, &user);

        // After revoke_role, is_role_expired returns false
        assert!(!client.is_role_expired(&role, &user));

        // No RoleExpiry key exists in storage
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

        // Initially, role should have no members
        let members_before = client.get_role_members(&role);
        assert!(members_before.is_empty());

        // Grant role to user1
        client
            .grant_role(&admin, &user1, &role, &None);

        // Check that user1 is in role members
        let members_after_first = client.get_role_members(&role);
        assert_eq!(members_after_first.len(), 1);
        assert!(members_after_first.contains(&user1));

        // Grant role to user2
        client
            .grant_role(&admin, &user2, &role, &None);

        // Check that both users are in role members
        let members_after_second = client.get_role_members(&role);
        assert_eq!(members_after_second.len(), 2);
        assert!(members_after_second.contains(&user1));
        assert!(members_after_second.contains(&user2));
    }

    // Issue #175: grant_role missing guards

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
        let past_ledger = 0u64;

        client
            .grant_role(&admin, &user, &role, &None);

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
        assert!(client.has_role(&user, &role));

        client.blacklist(&admin, &user, &None::<String>, &None);
        assert!(!client.has_role(&user, &role));

        client.unblacklist(&admin, &user);
        assert!(client.has_role(&user, &role));
    }

    #[test]
    fn test_get_roles_for_address_populated_after_grant() {
        let (env, admin, client) = setup();
        let user = Address::generate(&env);
        let role1 = String::from_str(&env, "editor");
        let role2 = String::from_str(&env, "viewer");

        // Initially, user should have no roles
        let roles_before = client.get_roles_for_address(&user);
        assert!(roles_before.is_empty());

        // Grant role1 to user
        client
            .grant_role(&admin, &user, &role1, &None);

        // Check that role1 is in user's roles
        let roles_after_first = client.get_roles_for_address(&user);
        assert_eq!(roles_after_first.len(), 1);
        assert!(roles_after_first.contains(&role1));

        // Grant role2 to user
        client
            .grant_role(&admin, &user, &role2, &None);

        // Check that both roles are in user's roles
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

        // Old admin should no longer be able to call super-admin functions.
        // Use the correct grant_role argument order: (admin, account, role, expires_in).
        let role = String::from_str(&env, "operator");
        let user = Address::generate(&env);
        assert_eq!(
            client.try_grant_role(&admin, &user, &role, &None),
            Err(Ok(AccessError::Unauthorized))
        );

        // New admin should be able to grant roles.
        assert!(client
            .try_grant_role(&new_admin, &user, &role, &None)
            .is_ok());
    }

    #[test]
    fn test_transfer_super_admin_to_self_succeeds() {
        // Edge case: transferring to self should be a no-op but not error
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
        // Verifies revoke_role removes the HasRole key rather than setting it to false,
        // so a subsequent grant_role on the same (role, target) pair succeeds.
        // grant_role uses signature (admin, account, role, expires_in).
        let (env, admin, client) = setup();
        let role = String::from_str(&env, "operator");
        let user = Address::generate(&env);
        client.grant_role(&admin, &user, &role, &None);
        assert!(client.has_role(&user, &role));
        client.revoke_role(&admin, &role, &user);
        assert!(!client.has_role(&user, &role));
        // Re-granting must succeed — if the key was set to false instead of removed,
        // has_role_internal would return false but the key would still exist,
        // and a future implementation checking .has() would wrongly block the grant.
        assert!(client.try_grant_role(&admin, &user, &role, &None).is_ok());
        assert!(client.has_role(&user, &role));
    }

    #[test]
    fn test_revoke_nonexistent_role_fails() {
        let (env, admin, client) = setup();
        let role = String::from_str(&env, "operator");
        let user = Address::generate(&env);
        // Never granted — should return RoleNotFound
        let result = client.try_revoke_role(&admin, &role, &user);
        assert_eq!(result, Err(Ok(AccessError::RoleNotFound)));
    }

    #[test]
    fn test_expire_role_removes_access() {
        let (env, admin, client) = setup();
        let role = String::from_str(&env, "operator");
        let user = Address::generate(&env);
        // Grant with a long expiry
        client.grant_role(&admin, &user, &role, &Some(9999));
        assert!(client.has_role(&user, &role));
        // Force-expire the role
        client.expire_role(&admin, &role, &user);
        assert!(!client.has_role(&user, &role));
    }

    #[test]
    fn test_expire_role_allows_regrant() {
        let (env, admin, client) = setup();
        let role = String::from_str(&env, "operator");
        let user = Address::generate(&env);
        client.grant_role(&admin, &user, &role, &Some(9999));
        client.expire_role(&admin, &role, &user);
        // Should be able to grant again
        assert!(client
            .try_grant_role(&admin, &user, &role, &Some(9999))
            .is_ok());
        assert!(client.has_role(&user, &role));
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

        // Grant role with short expiry
        client.grant_role(&admin, &user, &role, &Some(10));

        // Verify user is initially in role members
        let members_before = client.get_role_members(&role);
        assert!(members_before.contains(&user));
        assert_eq!(members_before.len(), 1);

        // Advance time past expiry
        env.ledger().set_timestamp(env.ledger().timestamp() + 20);

        // has_role correctly returns false
        assert!(!client.has_role(&user, &role));

        // get_role_members should not contain the expired user
        let members_after = client.get_role_members(&role);
        assert!(!members_after.contains(&user));
        assert!(members_after.is_empty());
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
    fn test_set_role_admin_unauthorized_fails() {
        let (env, _admin, client) = setup();
        let role = String::from_str(&env, "operator");
        let attacker = Address::generate(&env);
        let target = Address::generate(&env);
        let result = client.try_set_role_admin(&attacker, &role, &target);
        assert_eq!(result, Err(Ok(AccessError::Unauthorized)));
    }
}

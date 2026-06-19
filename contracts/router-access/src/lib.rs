#![no_std]

//! Role-based access control for the Stellar Router suite.
//!
//! A role grant can be permanent or expire at an absolute ledger timestamp.
//! Role checks also respect blacklist state and parent-role inheritance.

use soroban_sdk::{
    contract, contracterror, contractimpl, contracttype, Address, Env, String, Symbol, Vec,
};

#[contracttype]
#[derive(Clone)]
pub enum DataKey {
    SuperAdmin,
    HasRole(String, Address),
    RoleExpiry(String, Address),
    RoleAdmin(String),
    RoleParent(String),
    RoleMember(String, u32),
    RoleMemberIndex(String, Address),
    RoleMemberCount(String),
    AddressRoles(Address),
    Blacklisted(Address),
    BlacklistReason(Address),
    BlacklistExpiry(Address),
}

#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
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

#[contract]
pub struct RouterAccess;

const MAX_HIERARCHY_DEPTH: u32 = 16;

#[contractimpl]
impl RouterAccess {
    pub fn initialize(env: Env, super_admin: Address) -> Result<(), AccessError> {
        if env.storage().instance().has(&DataKey::SuperAdmin) {
            return Err(AccessError::AlreadyInitialized);
        }

        env.storage()
            .instance()
            .set(&DataKey::SuperAdmin, &super_admin);
        Ok(())
    }

    pub fn super_admin(env: Env) -> Result<Address, AccessError> {
        env.storage()
            .instance()
            .get(&DataKey::SuperAdmin)
            .ok_or(AccessError::NotInitialized)
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
            (Symbol::new(&env, router_common::EVENT_ADMIN_TRANSFERRED),),
            (current, new_admin),
        );
        Ok(())
    }

    pub fn grant_role(
        env: Env,
        caller: Address,
        role: String,
        account: Address,
        expires_at: Option<u64>,
    ) -> Result<(), AccessError> {
        caller.require_auth();
        Self::require_role_manager(&env, &caller, &role)?;
        Self::grant_role_internal(&env, &role, &account, expires_at)
    }

    pub fn revoke_role(
        env: Env,
        caller: Address,
        role: String,
        account: Address,
    ) -> Result<(), AccessError> {
        caller.require_auth();
        Self::require_role_manager(&env, &caller, &role)?;
        Self::revoke_role_internal(&env, &role, &account)
    }

    pub fn has_role(env: Env, role: String, account: Address) -> bool {
        if Self::is_blacklisted_internal(&env, &account) {
            return false;
        }
        Self::has_role_internal(&env, &role, &account)
    }

    pub fn is_role_expired(env: Env, role: String, account: Address) -> bool {
        Self::role_is_expired(&env, &role, &account)
    }

    pub fn get_role_expiry(env: Env, role: String, account: Address) -> Option<u64> {
        env.storage()
            .instance()
            .get::<DataKey, u64>(&DataKey::RoleExpiry(role, account))
    }

    pub fn expire_role(
        env: Env,
        caller: Address,
        role: String,
        account: Address,
    ) -> Result<(), AccessError> {
        caller.require_auth();
        Self::require_super_admin(&env, &caller)?;
        Self::remove_role_grant(&env, &role, &account);
        env.events()
            .publish((Symbol::new(&env, "role_expired"),), (role, account));
        Ok(())
    }

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

    pub fn get_role_admin(env: Env, role: String) -> Option<Address> {
        env.storage()
            .instance()
            .get::<DataKey, Address>(&DataKey::RoleAdmin(role))
    }

    pub fn is_role_admin(env: Env, role: String, addr: Address) -> bool {
        Self::get_role_admin(env, role)
            .map(|admin| admin == addr)
            .unwrap_or(false)
    }

    pub fn set_role_parent(
        env: Env,
        caller: Address,
        role: String,
        parent_role: String,
    ) -> Result<(), AccessError> {
        caller.require_auth();
        Self::require_super_admin(&env, &caller)?;
        if role == parent_role || Self::is_ancestor(&env, &parent_role, &role) {
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

    pub fn remove_role_parent(env: Env, caller: Address, role: String) -> Result<(), AccessError> {
        caller.require_auth();
        Self::require_super_admin(&env, &caller)?;
        env.storage()
            .instance()
            .remove(&DataKey::RoleParent(role.clone()));
        env.events().publish(
            (Symbol::new(&env, router_common::EVENT_ROLE_PARENT_REMOVED),),
            role,
        );
        Ok(())
    }

    pub fn get_role_parent(env: Env, role: String) -> Option<String> {
        env.storage()
            .instance()
            .get::<DataKey, String>(&DataKey::RoleParent(role))
    }

    pub fn blacklist(
        env: Env,
        caller: Address,
        target: Address,
        reason: Option<String>,
        expires_at: Option<u64>,
    ) -> Result<(), AccessError> {
        caller.require_auth();
        Self::require_super_admin(&env, &caller)?;

        let super_admin = Self::super_admin(env.clone())?;
        if target == super_admin {
            return Err(AccessError::CannotBlacklistAdmin);
        }

        env.storage()
            .instance()
            .set(&DataKey::Blacklisted(target.clone()), &true);
        if let Some(reason_value) = reason.clone() {
            env.storage()
                .instance()
                .set(&DataKey::BlacklistReason(target.clone()), &reason_value);
        } else {
            env.storage()
                .instance()
                .remove(&DataKey::BlacklistReason(target.clone()));
        }

        if let Some(expiry) = expires_at {
            env.storage()
                .instance()
                .set(&DataKey::BlacklistExpiry(target.clone()), &expiry);
        } else {
            env.storage()
                .instance()
                .remove(&DataKey::BlacklistExpiry(target.clone()));
        }

        env.events().publish(
            (Symbol::new(&env, router_common::EVENT_ADDRESS_BLACKLISTED),),
            (target, reason, expires_at),
        );
        Ok(())
    }

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

    pub fn get_role_members(env: Env, role: String, offset: u32, limit: u32) -> Vec<Address> {
        let mut out = Vec::new(&env);
        if limit == 0 {
            return out;
        }

        let total = env
            .storage()
            .instance()
            .get::<DataKey, u32>(&DataKey::RoleMemberCount(role.clone()))
            .unwrap_or(0);

        let mut seen_active = 0u32;
        let mut i = 0u32;
        while i < total && out.len() < limit {
            if let Some(member) = env
                .storage()
                .instance()
                .get::<DataKey, Address>(&DataKey::RoleMember(role.clone(), i))
            {
                if Self::has_direct_active_role(&env, &role, &member) {
                    if seen_active >= offset {
                        out.push_back(member);
                    }
                    seen_active += 1;
                }
            }
            i += 1;
        }

        out
    }

    pub fn get_roles_for_address(env: Env, account: Address) -> Vec<String> {
        let roles: Vec<String> = env
            .storage()
            .instance()
            .get(&DataKey::AddressRoles(account.clone()))
            .unwrap_or_else(|| Vec::new(&env));
        let mut active = Vec::new(&env);
        for role in roles.iter() {
            if Self::has_direct_active_role(&env, &role, &account) {
                active.push_back(role);
            }
        }
        active
    }

    pub fn grant_role_batch(
        env: Env,
        caller: Address,
        role: String,
        accounts: Vec<Address>,
        expires_at: Option<u64>,
    ) -> Result<Vec<Result<(), AccessError>>, AccessError> {
        caller.require_auth();
        Self::require_role_manager(&env, &caller, &role)?;
        let mut results = Vec::new(&env);
        for account in accounts.iter() {
            results.push_back(Self::grant_role_internal(&env, &role, &account, expires_at));
        }
        Ok(results)
    }

    pub fn revoke_role_batch(
        env: Env,
        caller: Address,
        role: String,
        accounts: Vec<Address>,
    ) -> Result<Vec<Result<(), AccessError>>, AccessError> {
        caller.require_auth();
        Self::require_role_manager(&env, &caller, &role)?;
        let mut results = Vec::new(&env);
        for account in accounts.iter() {
            results.push_back(Self::revoke_role_internal(&env, &role, &account));
        }
        Ok(results)
    }

    pub fn bulk_revoke_role(
        env: Env,
        caller: Address,
        role: String,
        accounts: Vec<Address>,
    ) -> Result<(), AccessError> {
        caller.require_auth();
        Self::require_role_manager(&env, &caller, &role)?;
        for account in accounts.iter() {
            Self::revoke_role_internal(&env, &role, &account)?;
        }
        Ok(())
    }

    fn require_super_admin(env: &Env, caller: &Address) -> Result<(), AccessError> {
        let admin = env
            .storage()
            .instance()
            .get::<DataKey, Address>(&DataKey::SuperAdmin)
            .ok_or(AccessError::NotInitialized)?;
        if &admin == caller {
            Ok(())
        } else {
            Err(AccessError::Unauthorized)
        }
    }

    fn require_role_manager(env: &Env, caller: &Address, role: &String) -> Result<(), AccessError> {
        if Self::is_blacklisted_internal(env, caller) {
            return Err(AccessError::Blacklisted);
        }
        if Self::require_super_admin(env, caller).is_ok() {
            return Ok(());
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

    fn grant_role_internal(
        env: &Env,
        role: &String,
        account: &Address,
        expires_at: Option<u64>,
    ) -> Result<(), AccessError> {
        if Self::is_blacklisted_internal(env, account) {
            return Err(AccessError::Blacklisted);
        }
        if Self::has_direct_active_role(env, role, account) {
            return Err(AccessError::AlreadyHasRole);
        }

        env.storage()
            .instance()
            .set(&DataKey::HasRole(role.clone(), account.clone()), &true);

        match expires_at {
            Some(expiry) => env
                .storage()
                .instance()
                .set(&DataKey::RoleExpiry(role.clone(), account.clone()), &expiry),
            None => env
                .storage()
                .instance()
                .remove(&DataKey::RoleExpiry(role.clone(), account.clone())),
        }

        Self::index_role_member(env, role, account);
        Self::index_address_role(env, role, account);
        env.events().publish(
            (Symbol::new(env, router_common::EVENT_ROLE_GRANTED),),
            (role.clone(), account.clone(), expires_at),
        );
        Ok(())
    }

    fn revoke_role_internal(
        env: &Env,
        role: &String,
        account: &Address,
    ) -> Result<(), AccessError> {
        let key = DataKey::HasRole(role.clone(), account.clone());
        if !env.storage().instance().has(&key) {
            return Err(AccessError::RoleNotFound);
        }
        Self::remove_role_grant(env, role, account);
        env.events().publish(
            (Symbol::new(env, router_common::EVENT_ROLE_REVOKED),),
            (role.clone(), account.clone()),
        );
        Ok(())
    }

    fn remove_role_grant(env: &Env, role: &String, account: &Address) {
        env.storage()
            .instance()
            .remove(&DataKey::HasRole(role.clone(), account.clone()));
        env.storage()
            .instance()
            .remove(&DataKey::RoleExpiry(role.clone(), account.clone()));
        env.storage()
            .instance()
            .remove(&DataKey::RoleMemberIndex(role.clone(), account.clone()));
        Self::remove_address_role(env, role, account);
    }

    fn index_role_member(env: &Env, role: &String, account: &Address) {
        if env
            .storage()
            .instance()
            .has(&DataKey::RoleMemberIndex(role.clone(), account.clone()))
        {
            return;
        }

        let count = env
            .storage()
            .instance()
            .get::<DataKey, u32>(&DataKey::RoleMemberCount(role.clone()))
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

    fn index_address_role(env: &Env, role: &String, account: &Address) {
        let mut roles: Vec<String> = env
            .storage()
            .instance()
            .get(&DataKey::AddressRoles(account.clone()))
            .unwrap_or_else(|| Vec::new(env));
        if !roles.iter().any(|existing| existing == *role) {
            roles.push_back(role.clone());
        }
        env.storage()
            .instance()
            .set(&DataKey::AddressRoles(account.clone()), &roles);
    }

    fn remove_address_role(env: &Env, role: &String, account: &Address) {
        let mut roles: Vec<String> = env
            .storage()
            .instance()
            .get(&DataKey::AddressRoles(account.clone()))
            .unwrap_or_else(|| Vec::new(env));
        if let Some(index) = roles.iter().position(|existing| existing == *role) {
            roles.remove(index as u32);
            env.storage()
                .instance()
                .set(&DataKey::AddressRoles(account.clone()), &roles);
        }
    }

    fn has_role_internal(env: &Env, role: &String, account: &Address) -> bool {
        let mut current = role.clone();
        let mut depth = 0u32;
        loop {
            if Self::has_direct_active_role(env, &current, account) {
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

    fn has_direct_active_role(env: &Env, role: &String, account: &Address) -> bool {
        let granted = env
            .storage()
            .instance()
            .get::<DataKey, bool>(&DataKey::HasRole(role.clone(), account.clone()))
            .unwrap_or(false);
        granted && !Self::role_is_expired(env, role, account)
    }

    fn role_is_expired(env: &Env, role: &String, account: &Address) -> bool {
        env.storage()
            .instance()
            .get::<DataKey, u64>(&DataKey::RoleExpiry(role.clone(), account.clone()))
            .map(|expiry| env.ledger().timestamp() >= expiry)
            .unwrap_or(false)
    }

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
}

#[cfg(test)]
mod tests {
    extern crate std;

    use super::*;
    use soroban_sdk::{
        testutils::{Address as _, Events, Ledger},
        Env, IntoVal,
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
    fn grant_role_supports_permanent_and_expiring_grants() {
        let (env, admin, client) = setup();
        let role = String::from_str(&env, "operator");
        let expiring = Address::generate(&env);
        let permanent = Address::generate(&env);

        client.grant_role(&admin, &role, &expiring, &Some(50));
        client.grant_role(&admin, &role, &permanent, &None);

        assert!(client.has_role(&role, &expiring));
        assert!(client.has_role(&role, &permanent));
        assert_eq!(client.get_role_expiry(&role, &expiring), Some(50));
        assert_eq!(client.get_role_expiry(&role, &permanent), None);

        env.ledger().set_timestamp(51);
        assert!(!client.has_role(&role, &expiring));
        assert!(client.has_role(&role, &permanent));
        assert!(client.is_role_expired(&role, &expiring));
    }

    #[test]
    fn role_granted_event_includes_expiry_field() {
        let (env, admin, client) = setup();
        let role = String::from_str(&env, "auditor");
        let user = Address::generate(&env);

        client.grant_role(&admin, &role, &user, &Some(123));

        let events = env.events().all();
        let last = events.last().unwrap();
        let topic: Symbol = last.1.get(0).unwrap().into_val(&env);
        assert_eq!(topic, Symbol::new(&env, router_common::EVENT_ROLE_GRANTED));
        let (emitted_role, emitted_user, emitted_expiry): (String, Address, Option<u64>) =
            last.2.into_val(&env);
        assert_eq!(emitted_role, role);
        assert_eq!(emitted_user, user);
        assert_eq!(emitted_expiry, Some(123));
    }

    #[test]
    fn expired_grant_can_be_regranted() {
        let (env, admin, client) = setup();
        let role = String::from_str(&env, "operator");
        let user = Address::generate(&env);

        client.grant_role(&admin, &role, &user, &Some(10));
        env.ledger().set_timestamp(11);

        assert!(!client.has_role(&role, &user));
        assert!(client
            .try_grant_role(&admin, &role, &user, &Some(100))
            .is_ok());
        assert!(client.has_role(&role, &user));
    }

    #[test]
    fn role_hierarchy_respects_expiring_parent_grants() {
        let (env, admin, client) = setup();
        let viewer = String::from_str(&env, "viewer");
        let editor = String::from_str(&env, "editor");
        let user = Address::generate(&env);

        client.set_role_parent(&admin, &viewer, &editor);
        client.grant_role(&admin, &editor, &user, &Some(20));

        assert!(client.has_role(&editor, &user));
        assert!(client.has_role(&viewer, &user));

        env.ledger().set_timestamp(21);
        assert!(!client.has_role(&editor, &user));
        assert!(!client.has_role(&viewer, &user));
    }

    #[test]
    fn blacklisted_accounts_cannot_receive_or_use_roles() {
        let (env, admin, client) = setup();
        let role = String::from_str(&env, "operator");
        let user = Address::generate(&env);

        client.grant_role(&admin, &role, &user, &None);
        assert!(client.has_role(&role, &user));

        client.blacklist(&admin, &user, &None::<String>, &None);
        assert!(!client.has_role(&role, &user));
        assert_eq!(
            client.try_grant_role(&admin, &role, &user, &None),
            Err(Ok(AccessError::Blacklisted))
        );

        client.unblacklist(&admin, &user);
        assert!(client.has_role(&role, &user));
    }

    #[test]
    fn blacklist_expiry_allows_future_role_grants() {
        let (env, admin, client) = setup();
        let role = String::from_str(&env, "operator");
        let user = Address::generate(&env);

        client.blacklist(&admin, &user, &None::<String>, &Some(25));
        assert!(client.is_blacklisted(&user));

        env.ledger().set_timestamp(26);
        assert!(!client.is_blacklisted(&user));
        assert!(client.try_grant_role(&admin, &role, &user, &None).is_ok());
    }

    #[test]
    fn role_admin_must_not_be_blacklisted() {
        let (env, admin, client) = setup();
        let role = String::from_str(&env, "operator");
        let role_admin = Address::generate(&env);
        let user = Address::generate(&env);

        client.set_role_admin(&admin, &role, &role_admin);
        assert!(client.is_role_admin(&role, &role_admin));
        client.blacklist(&admin, &role_admin, &None::<String>, &None);

        assert_eq!(
            client.try_grant_role(&role_admin, &role, &user, &None),
            Err(Ok(AccessError::Blacklisted))
        );
        assert_eq!(
            client.try_revoke_role(&role_admin, &role, &user),
            Err(Ok(AccessError::Blacklisted))
        );
    }

    #[test]
    fn revoke_removes_role_and_allows_regrant() {
        let (env, admin, client) = setup();
        let role = String::from_str(&env, "operator");
        let user = Address::generate(&env);

        client.grant_role(&admin, &role, &user, &Some(100));
        client.revoke_role(&admin, &role, &user);

        assert!(!client.has_role(&role, &user));
        assert_eq!(client.get_role_expiry(&role, &user), None);
        assert!(client.try_grant_role(&admin, &role, &user, &None).is_ok());
    }

    #[test]
    fn role_member_and_address_indexes_filter_inactive_grants() {
        let (env, admin, client) = setup();
        let role = String::from_str(&env, "operator");
        let active = Address::generate(&env);
        let expired = Address::generate(&env);

        client.grant_role(&admin, &role, &active, &None);
        client.grant_role(&admin, &role, &expired, &Some(5));
        env.ledger().set_timestamp(6);

        let members = client.get_role_members(&role, &0, &50);
        assert_eq!(members.len(), 1);
        assert!(members.contains(&active));
        assert!(!members.contains(&expired));

        let expired_roles = client.get_roles_for_address(&expired);
        assert!(expired_roles.is_empty());
    }

    #[test]
    fn batch_grants_report_partial_results() {
        let (env, admin, client) = setup();
        let role = String::from_str(&env, "operator");
        let u1 = Address::generate(&env);
        let u2 = Address::generate(&env);
        let accounts = soroban_sdk::vec![&env, u1.clone(), u2.clone()];

        client.grant_role(&admin, &role, &u1, &None);
        let results = client.grant_role_batch(&admin, &role, &accounts, &None);

        assert_eq!(results.get(0).unwrap(), Err(AccessError::AlreadyHasRole));
        assert_eq!(results.get(1).unwrap(), Ok(()));
    }

    #[test]
    fn role_parent_cycle_is_rejected() {
        let (env, admin, client) = setup();
        let a = String::from_str(&env, "a");
        let b = String::from_str(&env, "b");

        client.set_role_parent(&admin, &b, &a);
        assert_eq!(
            client.try_set_role_parent(&admin, &a, &b),
            Err(Ok(AccessError::HierarchyCycle))
        );
    }
}

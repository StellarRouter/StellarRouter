/// Failure scenario tests for stellar-router.
///
/// Verifies that all contracts return the correct errors for invalid inputs,
/// unauthorized callers, and edge cases. All tests run in the Soroban test
/// environment — no testnet required.
extern crate std;

use soroban_sdk::{
    testutils::{Address as _, Ledger},
    Address, Bytes, Env, String, Vec,
};

use router_access::{AccessError, RouterAccess, RouterAccessClient};
use router_core::{RouterCore, RouterCoreClient, RouterError};
use router_middleware::{MiddlewareError, RouterMiddleware, RouterMiddlewareClient};
use router_registry::{RegistryError, RouterRegistry, RouterRegistryClient};
use router_timelock::{RouterTimelock, RouterTimelockClient, TimelockError};

// ── Helpers ───────────────────────────────────────────────────────────────────

fn make_env() -> (Env, Address) {
    let env = Env::default();
    env.mock_all_auths();
    env.ledger().with_mut(|l| l.timestamp = 1000);
    let admin = Address::generate(&env);
    (env, admin)
}

// ── router-core failure scenarios ─────────────────────────────────────────────

#[test]
fn test_core_route_not_found() {
    let (env, admin) = make_env();
    let id = env.register_contract(None, RouterCore);
    let client = RouterCoreClient::new(&env, &id);
    client.initialize(&admin);

    let result = client.try_resolve(&String::from_str(&env, "nonexistent"));
    assert_eq!(result, Err(Ok(RouterError::RouteNotFound)));
}

#[test]
fn test_core_router_paused_blocks_all_routes() {
    let (env, admin) = make_env();
    let id = env.register_contract(None, RouterCore);
    let client = RouterCoreClient::new(&env, &id);
    client.initialize(&admin);

    let name = String::from_str(&env, "oracle");
    let addr = Address::generate(&env);
    client.register_route(&admin, &name, &addr, &None);

    client.set_paused(&admin, &true);

    let result = client.try_resolve(&name);
    assert_eq!(result, Err(Ok(RouterError::RouterPaused)));
}

#[test]
fn test_core_route_paused_blocks_specific_route() {
    let (env, admin) = make_env();
    let id = env.register_contract(None, RouterCore);
    let client = RouterCoreClient::new(&env, &id);
    client.initialize(&admin);

    let name = String::from_str(&env, "oracle");
    let addr = Address::generate(&env);
    client.register_route(&admin, &name, &addr, &None);
    client.set_route_paused(&admin, &name, &true);

    let result = client.try_resolve(&name);
    assert_eq!(result, Err(Ok(RouterError::RoutePaused)));
}

#[test]
fn test_core_unauthorized_register_fails() {
    let (env, _admin) = make_env();
    let id = env.register_contract(None, RouterCore);
    let client = RouterCoreClient::new(&env, &id);
    let admin = Address::generate(&env);
    client.initialize(&admin);

    let attacker = Address::generate(&env);
    let result = client.try_register_route(
        &attacker,
        &String::from_str(&env, "oracle"),
        &Address::generate(&env),
        &None,
    );
    assert_eq!(result, Err(Ok(RouterError::Unauthorized)));
}

#[test]
fn test_core_duplicate_route_fails() {
    let (env, admin) = make_env();
    let id = env.register_contract(None, RouterCore);
    let client = RouterCoreClient::new(&env, &id);
    client.initialize(&admin);

    let name = String::from_str(&env, "oracle");
    let addr = Address::generate(&env);
    client.register_route(&admin, &name, &addr, &None);

    let result = client.try_register_route(&admin, &name, &addr, &None);
    assert_eq!(result, Err(Ok(RouterError::RouteAlreadyExists)));
}

#[test]
fn test_core_empty_route_name_fails() {
    let (env, admin) = make_env();
    let id = env.register_contract(None, RouterCore);
    let client = RouterCoreClient::new(&env, &id);
    client.initialize(&admin);

    let result = client.try_register_route(
        &admin,
        &String::from_str(&env, ""),
        &Address::generate(&env),
        &None,
    );
    assert_eq!(result, Err(Ok(RouterError::InvalidRouteName)));
}

// ── router-registry failure scenarios ────────────────────────────────────────

#[test]
fn test_registry_version_must_increase() {
    let (env, admin) = make_env();
    let id = env.register_contract(None, RouterRegistry);
    let client = RouterRegistryClient::new(&env, &id);
    client.initialize(&admin);

    let name = String::from_str(&env, "oracle");
    let addr = Address::generate(&env);
    client.register(&admin, &name, &addr, &5);

    let result = client.try_register(&admin, &name, &addr, &3);
    assert_eq!(result, Err(Ok(RegistryError::InvalidVersion)));
}

#[test]
fn test_registry_get_unknown_name_fails() {
    let (env, admin) = make_env();
    let id = env.register_contract(None, RouterRegistry);
    let client = RouterRegistryClient::new(&env, &id);
    client.initialize(&admin);

    let result = client.try_get_latest(&String::from_str(&env, "unknown"));
    assert_eq!(result, Err(Ok(RegistryError::NotFound)));
}

#[test]
fn test_registry_double_deprecate_fails() {
    let (env, admin) = make_env();
    let id = env.register_contract(None, RouterRegistry);
    let client = RouterRegistryClient::new(&env, &id);
    client.initialize(&admin);

    let name = String::from_str(&env, "oracle");
    client.register(&admin, &name, &Address::generate(&env), &1);
    client.deprecate(&admin, &name, &1, &None::<String>);

    let result = client.try_deprecate(&admin, &name, &1, &None::<String>);
    assert_eq!(result, Err(Ok(RegistryError::AlreadyDeprecated)));
}

#[test]
fn test_registry_unauthorized_register_fails() {
    let (env, admin) = make_env();
    let id = env.register_contract(None, RouterRegistry);
    let client = RouterRegistryClient::new(&env, &id);
    client.initialize(&admin);

    let attacker = Address::generate(&env);
    let result = client.try_register(
        &attacker,
        &String::from_str(&env, "oracle"),
        &Address::generate(&env),
        &1,
    );
    assert_eq!(result, Err(Ok(RegistryError::Unauthorized)));
}

// ── router-access failure scenarios ──────────────────────────────────────────

#[test]
fn test_access_unauthorized_grant_fails() {
    let (env, admin) = make_env();
    let id = env.register_contract(None, RouterAccess);
    let client = RouterAccessClient::new(&env, &id);
    client.initialize(&admin);

    let attacker = Address::generate(&env);
    // grant_role(admin, account, role, expires_at)
    let result = client.try_grant_role(
        &attacker,
        &Address::generate(&env),
        &String::from_str(&env, "operator"),
        &None,
    );
    assert_eq!(result, Err(Ok(AccessError::Unauthorized)));
}

#[test]
fn test_access_blacklisted_address_cannot_receive_role() {
    let (env, admin) = make_env();
    let id = env.register_contract(None, RouterAccess);
    let client = RouterAccessClient::new(&env, &id);
    client.initialize(&admin);

    let user = Address::generate(&env);
    // blacklist(caller, target, reason, expires_at)
    client.blacklist(&admin, &user, &None::<String>, &None);

    let result = client.try_grant_role(&admin, &user, &String::from_str(&env, "operator"), &None);
    assert_eq!(result, Err(Ok(AccessError::Blacklisted)));
}

#[test]
fn test_access_cannot_blacklist_super_admin() {
    let (env, admin) = make_env();
    let id = env.register_contract(None, RouterAccess);
    let client = RouterAccessClient::new(&env, &id);
    client.initialize(&admin);

    // blacklist(caller, target, reason, expires_at)
    let result = client.try_blacklist(&admin, &admin, &None::<String>, &None);
    assert_eq!(result, Err(Ok(AccessError::CannotBlacklistAdmin)));
}

#[test]
fn test_access_double_grant_fails() {
    let (env, admin) = make_env();
    let id = env.register_contract(None, RouterAccess);
    let client = RouterAccessClient::new(&env, &id);
    client.initialize(&admin);

    let role = String::from_str(&env, "operator");
    let user = Address::generate(&env);
    // grant_role(admin, account, role, expires_at)
    client.grant_role(&admin, &user, &role, &None);

    let result = client.try_grant_role(&admin, &user, &role, &None);
    assert_eq!(result, Err(Ok(AccessError::AlreadyHasRole)));
}

// ── router-middleware failure scenarios ───────────────────────────────────────

#[test]
fn test_middleware_rate_limit_exceeded() {
    let (env, admin) = make_env();
    let id = env.register_contract(None, RouterMiddleware);
    let client = RouterMiddlewareClient::new(&env, &id);
    client.initialize(&admin);

    let route = String::from_str(&env, "oracle/price");
    let caller = Address::generate(&env);
    // max 2 calls per 60s
    client.configure_route(&admin, &route, &2, &60, &true, &0, &0, &0);

    client.pre_call(&caller, &route);
    client.pre_call(&caller, &route);

    let result = client.try_pre_call(&caller, &route);
    assert_eq!(result, Err(Ok(MiddlewareError::RateLimitExceeded)));
}

#[test]
fn test_middleware_disabled_route_blocked() {
    let (env, admin) = make_env();
    let id = env.register_contract(None, RouterMiddleware);
    let client = RouterMiddlewareClient::new(&env, &id);
    client.initialize(&admin);

    let route = String::from_str(&env, "oracle/price");
    let caller = Address::generate(&env);
    client.configure_route(&admin, &route, &0, &0, &false, &0, &0, &0);

    let result = client.try_pre_call(&caller, &route);
    assert_eq!(result, Err(Ok(MiddlewareError::RouteDisabled)));
}

#[test]
fn test_middleware_globally_disabled_blocks_all() {
    let (env, admin) = make_env();
    let id = env.register_contract(None, RouterMiddleware);
    let client = RouterMiddlewareClient::new(&env, &id);
    client.initialize(&admin);

    client.set_global_enabled(&admin, &false);

    let result = client.try_pre_call(
        &Address::generate(&env),
        &String::from_str(&env, "any/route"),
    );
    assert_eq!(result, Err(Ok(MiddlewareError::MiddlewareDisabled)));
}

#[test]
fn test_middleware_circuit_breaker_open() {
    let (env, admin) = make_env();
    let id = env.register_contract(None, RouterMiddleware);
    let client = RouterMiddlewareClient::new(&env, &id);
    client.initialize(&admin);

    let route = String::from_str(&env, "oracle/price");
    let caller = Address::generate(&env);
    // failure_threshold = 1, recovery = 60s
    client.configure_route(&admin, &route, &0, &0, &true, &1, &60, &0);

    // Trip the circuit
    client.post_call(&caller, &route, &false);

    let result = client.try_pre_call(&caller, &route);
    assert_eq!(result, Err(Ok(MiddlewareError::CircuitOpen)));
}

#[test]
fn test_middleware_unauthorized_configure_fails() {
    let (env, admin) = make_env();
    let id = env.register_contract(None, RouterMiddleware);
    let client = RouterMiddlewareClient::new(&env, &id);
    client.initialize(&admin);

    let attacker = Address::generate(&env);
    let result = client.try_configure_route(
        &attacker,
        &String::from_str(&env, "oracle/price"),
        &0,
        &0,
        &true,
        &0,
        &0,
        &0,
    );
    assert_eq!(result, Err(Ok(MiddlewareError::Unauthorized)));
}

// ── router-timelock failure scenarios ────────────────────────────────────────

// queue signature: (proposer, description, target, delay, grace_period_seconds, deps)
// execute returns NotReady (not TooEarly) when called before eta

#[test]
fn test_timelock_execute_too_early() {
    let (env, admin) = make_env();
    let id = env.register_contract(None, RouterTimelock);
    let client = RouterTimelockClient::new(&env, &id);
    client.initialize(&admin, &3600);

    let deps: Vec<Bytes> = Vec::new(&env);
    let op_id = client.queue(
        &admin,
        &String::from_str(&env, "upgrade oracle"),
        &Address::generate(&env),
        &3600,
        &86_400u64, // grace_period_seconds
        &deps,
    );

    let result = client.try_execute(&admin, &op_id);
    assert_eq!(result, Err(Ok(TimelockError::NotReady)));
}

#[test]
fn test_timelock_delay_below_minimum_fails() {
    let (env, admin) = make_env();
    let id = env.register_contract(None, RouterTimelock);
    let client = RouterTimelockClient::new(&env, &id);
    client.initialize(&admin, &3600);

    let deps: Vec<Bytes> = Vec::new(&env);
    let result = client.try_queue(
        &admin,
        &String::from_str(&env, "upgrade oracle"),
        &Address::generate(&env),
        &100, // below min_delay of 3600
        &86_400u64,
        &deps,
    );
    assert_eq!(result, Err(Ok(TimelockError::DelayTooShort)));
}

#[test]
fn test_timelock_execute_cancelled_op_fails() {
    let (env, admin) = make_env();
    let id = env.register_contract(None, RouterTimelock);
    let client = RouterTimelockClient::new(&env, &id);
    client.initialize(&admin, &3600);

    let deps: Vec<Bytes> = Vec::new(&env);
    let op_id = client.queue(
        &admin,
        &String::from_str(&env, "upgrade oracle"),
        &Address::generate(&env),
        &3600,
        &86_400u64,
        &deps,
    );
    client.cancel(&admin, &op_id);

    env.ledger().with_mut(|l| l.timestamp += 3601);
    let result = client.try_execute(&admin, &op_id);
    assert_eq!(result, Err(Ok(TimelockError::Cancelled)));
}

#[test]
fn test_timelock_double_execute_fails() {
    let (env, admin) = make_env();
    let id = env.register_contract(None, RouterTimelock);
    let client = RouterTimelockClient::new(&env, &id);
    client.initialize(&admin, &3600);

    let deps: Vec<Bytes> = Vec::new(&env);
    let op_id = client.queue(
        &admin,
        &String::from_str(&env, "upgrade oracle"),
        &Address::generate(&env),
        &3600,
        &86_400u64,
        &deps,
    );
    env.ledger().with_mut(|l| l.timestamp += 3601);
    client.execute(&admin, &op_id);

    let result = client.try_execute(&admin, &op_id);
    assert_eq!(result, Err(Ok(TimelockError::AlreadyExecuted)));
}

#[test]
fn test_timelock_unauthorized_queue_fails() {
    let (env, admin) = make_env();
    let id = env.register_contract(None, RouterTimelock);
    let client = RouterTimelockClient::new(&env, &id);
    client.initialize(&admin, &3600);

    let attacker = Address::generate(&env);
    let deps: Vec<Bytes> = Vec::new(&env);
    let result = client.try_queue(
        &attacker,
        &String::from_str(&env, "malicious"),
        &Address::generate(&env),
        &3600,
        &86_400u64,
        &deps,
    );
    assert_eq!(result, Err(Ok(TimelockError::Unauthorized)));
}

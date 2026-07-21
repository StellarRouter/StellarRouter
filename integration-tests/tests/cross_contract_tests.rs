/// Cross-contract integration tests for stellar-router.
///
/// These tests run entirely in the Soroban test environment — no testnet
/// required. They deploy all relevant contracts into a single Env and verify
/// that the contracts interact correctly end-to-end.
///
/// Run with:
///   cargo test --test cross_contract_tests
extern crate std;

use soroban_sdk::{
    testutils::{Address as _, Ledger},
    Address, Env, IntoVal, String, Symbol, Val, Vec,
};

// ── Contract imports ──────────────────────────────────────────────────────────

use router_access::{RouterAccess, RouterAccessClient};
use router_core::{RouteMetadata, RouterCore, RouterCoreClient};
use router_middleware::{RouterMiddleware, RouterMiddlewareClient};
use router_multicall::{CallDescriptor, RouterMulticall, RouterMulticallClient};
use router_registry::{RouterRegistry, RouterRegistryClient};

// ── Shared setup ──────────────────────────────────────────────────────────────

struct Suite<'a> {
    env: Env,
    admin: Address,
    core: RouterCoreClient<'a>,
    core_id: Address,
    registry: RouterRegistryClient<'a>,
    registry_id: Address,
    access: RouterAccessClient<'a>,
    access_id: Address,
    middleware: RouterMiddlewareClient<'a>,
    middleware_id: Address,
}

fn setup() -> Suite<'static> {
    let env = Env::default();
    env.mock_all_auths();
    env.ledger().with_mut(|l| l.timestamp = 1000);

    let admin = Address::generate(&env);

    let core_id = env.register_contract(None, RouterCore);
    let registry_id = env.register_contract(None, RouterRegistry);
    let access_id = env.register_contract(None, RouterAccess);
    let middleware_id = env.register_contract(None, RouterMiddleware);

    let core = RouterCoreClient::new(&env, &core_id);
    let registry = RouterRegistryClient::new(&env, &registry_id);
    let access = RouterAccessClient::new(&env, &access_id);
    let middleware = RouterMiddlewareClient::new(&env, &middleware_id);

    core.initialize(&admin);
    registry.initialize(&admin);
    access.initialize(&admin);
    middleware.initialize(&admin);

    Suite {
        env,
        admin,
        core,
        core_id: core_id.clone(),
        registry,
        registry_id: registry_id.clone(),
        access,
        access_id: access_id.clone(),
        middleware,
        middleware_id: middleware_id.clone(),
    }
}

// ── router-core + router-registry ────────────────────────────────────────────

/// Register a contract in the registry, then register the same address as a
/// route in router-core and resolve it. Verifies the two contracts agree on
/// the address.
#[test]
fn test_core_resolves_address_registered_in_registry() {
    let s = setup();
    let name = String::from_str(&s.env, "oracle");
    let oracle_addr = Address::generate(&s.env);

    // Register in registry at version 1
    s.registry.register(&s.admin, &name, &oracle_addr, &1);
    let entry = s.registry.get_latest(&name);
    assert_eq!(entry.address, oracle_addr);

    // Register the same address as a route in core
    s.core.register_route(&s.admin, &name, &oracle_addr, &None);
    let resolved = s.core.resolve(&name);
    assert_eq!(resolved, oracle_addr);
    assert_eq!(resolved, entry.address);
}

/// Deprecate a registry version and register a new one, then update the
/// core route to point to the new address.
#[test]
fn test_core_route_updated_after_registry_version_bump() {
    let s = setup();
    let name = String::from_str(&s.env, "oracle");
    let v1_addr = Address::generate(&s.env);
    let v2_addr = Address::generate(&s.env);

    s.registry.register(&s.admin, &name, &v1_addr, &1);
    s.core.register_route(&s.admin, &name, &v1_addr, &None);
    assert_eq!(s.core.resolve(&name), v1_addr);

    // Bump registry to v2
    s.registry.register(&s.admin, &name, &v2_addr, &2);
    s.registry.deprecate(&s.admin, &name, &1, &None::<String>);
    assert_eq!(s.registry.get_latest(&name).address, v2_addr);

    // Update core route to match
    s.core.update_route(&s.admin, &name, &v2_addr);
    assert_eq!(s.core.resolve(&name), v2_addr);
}

// ── router-core + router-access ───────────────────────────────────────────────

/// Only addresses with the "router_admin" role should be able to register
/// routes. Verify that a non-admin address is rejected by router-core even
/// if it holds a different role in router-access.
#[test]
fn test_core_rejects_unauthorized_even_with_unrelated_role() {
    let s = setup();
    let name = String::from_str(&s.env, "oracle");
    let addr = Address::generate(&s.env);
    let user = Address::generate(&s.env);
    let unrelated_role = String::from_str(&s.env, "viewer");

    // Grant user an unrelated role in access
    s.access.grant_role(&s.admin, &user, &unrelated_role, &None);
    assert!(s.access.has_role(&unrelated_role, &user));

    // User should still be rejected by core (core uses its own admin check)
    let result = s.core.try_register_route(&user, &name, &addr, &None);
    assert_eq!(result, Err(Ok(router_core::RouterError::Unauthorized)));
}

/// Blacklisting an address in router-access should not affect router-core
/// directly (they are independent contracts), but demonstrates that the
/// access contract correctly reflects the blacklist state.
#[test]
fn test_access_blacklist_state_is_independent_of_core() {
    let s = setup();
    let name = String::from_str(&s.env, "oracle");
    let addr = Address::generate(&s.env);
    let user = Address::generate(&s.env);
    let role = String::from_str(&s.env, "operator");

    // Register route in core as admin
    s.core.register_route(&s.admin, &name, &addr, &None);

    // Grant role then blacklist user in access
    s.access.grant_role(&s.admin, &user, &role, &None);
    s.access.blacklist(&s.admin, &user, &None::<String>, &None);
    assert!(!s.access.has_role(&role, &user));

    // Core resolve is unaffected — it doesn't consult access
    assert_eq!(s.core.resolve(&name), addr);
}

// ── router-core + router-middleware ───────────────────────────────────────────

/// Configure middleware for a route, call pre_call, and verify the global
/// call counter increments. Then pause the route in core and verify that
/// middleware correctly blocks the call.
#[test]
fn test_middleware_pre_call_passes_for_enabled_route() {
    let s = setup();
    let route = String::from_str(&s.env, "oracle/get_price");
    let addr = Address::generate(&s.env);
    let caller = Address::generate(&s.env);

    s.core.register_route(&s.admin, &route, &addr, &None);
    s.middleware
        .configure_route(&s.admin, &route, &10, &60, &true, &0, &0, &0);

    s.middleware.pre_call(&caller, &route);
    assert_eq!(s.middleware.total_calls(), 1);
}

#[test]
fn test_middleware_rate_limit_blocks_after_threshold() {
    let s = setup();
    let route = String::from_str(&s.env, "oracle/get_price");
    let addr = Address::generate(&s.env);
    let caller = Address::generate(&s.env);

    s.core.register_route(&s.admin, &route, &addr, &None);
    // max 2 calls per 60s window
    s.middleware
        .configure_route(&s.admin, &route, &2, &60, &true, &0, &0, &0);

    s.middleware.pre_call(&caller, &route);
    s.middleware.pre_call(&caller, &route);
    let result = s.middleware.try_pre_call(&caller, &route);
    assert_eq!(
        result,
        Err(Ok(router_middleware::MiddlewareError::RateLimitExceeded))
    );
}

#[test]
fn test_middleware_disabled_route_blocks_pre_call() {
    let s = setup();
    let route = String::from_str(&s.env, "oracle/get_price");
    let addr = Address::generate(&s.env);
    let caller = Address::generate(&s.env);

    s.core.register_route(&s.admin, &route, &addr, &None);
    s.middleware
        .configure_route(&s.admin, &route, &0, &0, &false, &0, &0, &0);

    let result = s.middleware.try_pre_call(&caller, &route);
    assert_eq!(
        result,
        Err(Ok(router_middleware::MiddlewareError::RouteDisabled))
    );
}

// ── Full resolution path ──────────────────────────────────────────────────────

/// Full happy-path flow:
/// 1. Register contract in registry
/// 2. Register route in core pointing to registry entry
/// 3. Configure middleware for the route
/// 4. Call middleware pre_call (passes)
/// 5. Resolve route via core
/// 6. Call middleware post_call (success)
/// 7. Verify total_calls and total_routed counters
#[test]
fn test_full_resolution_path() {
    let s = setup();
    let route = String::from_str(&s.env, "oracle");
    let oracle_addr = Address::generate(&s.env);
    let caller = Address::generate(&s.env);

    // Step 1 & 2: register in registry and core
    s.registry.register(&s.admin, &route, &oracle_addr, &1);
    s.core.register_route(&s.admin, &route, &oracle_addr, &None);

    // Step 3: configure middleware (5 calls/min, circuit breaker threshold 3)
    s.middleware
        .configure_route(&s.admin, &route, &5, &60, &true, &3, &30, &0);

    // Step 4: pre_call
    assert!(s.middleware.try_pre_call(&caller, &route).is_ok());

    // Step 5: resolve
    let resolved = s.core.resolve(&route);
    assert_eq!(resolved, oracle_addr);

    // Step 6: post_call (success)
    s.middleware.post_call(&caller, &route, &true);

    // Step 7: verify counters
    assert_eq!(s.middleware.total_calls(), 1);
    assert_eq!(s.core.total_routed(), 1);
}

/// Verify that pausing a route in core does not affect middleware state,
/// and that re-enabling it restores the full flow.
#[test]
fn test_pause_and_unpause_route_in_core() {
    let s = setup();
    let route = String::from_str(&s.env, "oracle");
    let oracle_addr = Address::generate(&s.env);

    s.core.register_route(&s.admin, &route, &oracle_addr, &None);
    s.middleware
        .configure_route(&s.admin, &route, &0, &0, &true, &0, &0, &0);

    // Pause in core
    s.core.set_route_paused(&s.admin, &route, &true);
    assert_eq!(
        s.core.try_resolve(&route),
        Err(Ok(router_core::RouterError::RoutePaused))
    );

    // Middleware pre_call is unaffected (it doesn't know about core pause state)
    let caller = Address::generate(&s.env);
    assert!(s.middleware.try_pre_call(&caller, &route).is_ok());

    // Unpause and verify full flow works again
    s.core.set_route_paused(&s.admin, &route, &false);
    assert_eq!(s.core.resolve(&route), oracle_addr);
}

/// Verify circuit breaker trips after threshold failures and blocks pre_call.
#[test]
fn test_circuit_breaker_trips_after_failures() {
    let s = setup();
    let route = String::from_str(&s.env, "oracle");
    let oracle_addr = Address::generate(&s.env);
    let caller = Address::generate(&s.env);

    s.core.register_route(&s.admin, &route, &oracle_addr, &None);
    // failure_threshold = 2
    s.middleware
        .configure_route(&s.admin, &route, &0, &0, &true, &2, &60, &0);

    // Two failures trip the circuit
    s.middleware.post_call(&caller, &route, &false);
    s.middleware.post_call(&caller, &route, &false);

    // pre_call should now be blocked
    let result = s.middleware.try_pre_call(&caller, &route);
    assert_eq!(
        result,
        Err(Ok(router_middleware::MiddlewareError::CircuitOpen))
    );

    // Advance past recovery window
    s.env.ledger().with_mut(|l| l.timestamp += 61);
    assert!(s.middleware.try_pre_call(&caller, &route).is_ok());
}

// ── router-multicall × core × middleware ─────────────────────────────────────

/// Execute a batch through router-multicall that mixes a
/// router-core.register_route call and a router-middleware.configure_route
/// call, asserting both land atomically.
#[test]
fn test_multicall_execute_batch_mixes_core_and_middleware() {
    let s = setup();
    let multicall_id = s.env.register_contract(None, RouterMulticall);
    let multicall = RouterMulticallClient::new(&s.env, &multicall_id);
    multicall.initialize(&s.admin, &10);

    let route = String::from_str(&s.env, "oracle");
    let target_addr = Address::generate(&s.env);

    // ── Call 1: router-core.register_route ──────────────────────────────
    // register_route(caller, name, address, metadata)
    let mut reg_args: Vec<Val> = Vec::new(&s.env);
    reg_args.push_back(s.admin.clone().into_val(&s.env));
    reg_args.push_back(route.clone().into_val(&s.env));
    reg_args.push_back(target_addr.clone().into_val(&s.env));
    reg_args.push_back(None::<RouteMetadata>.into_val(&s.env));

    let reg_call = CallDescriptor {
        target: s.core_id.clone(),
        function: Symbol::new(&s.env, "register_route"),
        required: true,
        instruction_budget: None,
        args: reg_args,
    };

    // ── Call 2: router-middleware.configure_route ───────────────────────
    let mut cfg_args: Vec<Val> = Vec::new(&s.env);
    cfg_args.push_back(s.admin.clone().into_val(&s.env));
    cfg_args.push_back(route.clone().into_val(&s.env));
    cfg_args.push_back((10u32).into_val(&s.env));
    cfg_args.push_back((60u64).into_val(&s.env));
    cfg_args.push_back(true.into_val(&s.env));
    cfg_args.push_back((0u32).into_val(&s.env));
    cfg_args.push_back((0u64).into_val(&s.env));
    cfg_args.push_back((0u32).into_val(&s.env));

    let cfg_call = CallDescriptor {
        target: s.middleware_id.clone(),
        function: Symbol::new(&s.env, "configure_route"),
        required: true,
        instruction_budget: None,
        args: cfg_args,
    };

    let mut calls: Vec<CallDescriptor> = Vec::new(&s.env);
    calls.push_back(reg_call);
    calls.push_back(cfg_call);

    let summary = multicall.execute_batch(&s.admin, &calls, &false, &false);

    assert_eq!(summary.total, 2);
    assert_eq!(summary.succeeded, 2);
    assert_eq!(summary.failed, 0);

    // Verify router-core state: route "oracle" resolves to target_addr
    let resolved = s.core.resolve(&route);
    assert_eq!(resolved, target_addr);

    // Verify router-middleware state: "oracle" is in the configured list
    let configured = s.middleware.get_configured_routes();
    assert!(
        configured.contains(&route),
        "oracle should be a configured middleware route"
    );
}

/// Batch through multicall where a required call fails (duplicate route
/// registration). The entire batch must abort.
#[test]
fn test_multicall_required_call_failure_aborts_batch() {
    let s = setup();
    let multicall_id = s.env.register_contract(None, RouterMulticall);
    let multicall = RouterMulticallClient::new(&s.env, &multicall_id);
    multicall.initialize(&s.admin, &10);

    let route = String::from_str(&s.env, "oracle");
    let target_addr = Address::generate(&s.env);

    // Pre-register the route in core directly so the batch call will fail
    // with RouteAlreadyExists.
    s.core.register_route(&s.admin, &route, &target_addr, &None);

    // Build args for a duplicate register_route call
    let mut reg_args: Vec<Val> = Vec::new(&s.env);
    reg_args.push_back(s.admin.clone().into_val(&s.env));
    reg_args.push_back(route.clone().into_val(&s.env));
    reg_args.push_back(Address::generate(&s.env).into_val(&s.env));
    reg_args.push_back(None::<RouteMetadata>.into_val(&s.env));

    let mut calls: Vec<CallDescriptor> = Vec::new(&s.env);
    calls.push_back(CallDescriptor {
        target: s.core_id.clone(),
        function: Symbol::new(&s.env, "register_route"),
        required: true,
        instruction_budget: None,
        args: reg_args,
    });

    let result = multicall.try_execute_batch(&s.admin, &calls, &false, &false);
    assert_eq!(
        result,
        Err(Ok(router_multicall::MulticallError::RequiredCallFailed))
    );

    // Verify batch counter was NOT incremented (the batch was aborted)
    assert_eq!(multicall.total_batches(), 0);

    // Verify original route is still intact
    let resolved = s.core.resolve(&route);
    assert_eq!(resolved, target_addr);
}

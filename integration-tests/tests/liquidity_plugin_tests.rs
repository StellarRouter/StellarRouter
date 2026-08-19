/// End-to-end integration test for the reference LiquidityPlugin example
/// (contracts/liquidity-plugin-example-fixed-rate), demonstrating the full
/// lifecycle described in docs/plugin-system.md:
///
/// 1. **Register** — the plugin is registered in router-registry under its
///    well-known name (`liquidity/example-fixed-rate`) and resolved back.
/// 2. **Route** — a router-core route points at the plugin and `resolve()`
///    returns the plugin address.
/// 3. **Quote / Swap** — the plugin is invoked through the resolved address:
///    `get_quote` computes the output and `execute_swap` enforces slippage.
///
/// These tests run entirely in the Soroban test environment — no testnet
/// required. Run with:
///   cargo test -p integration-tests --test liquidity_plugin_tests
extern crate std;

use soroban_sdk::{
    testutils::{Address as _, Events, Ledger},
    Address, Env, String, Symbol, TryFromVal,
};

use liquidity_plugin_example_fixed_rate::{
    FixedRateLiquidityPlugin, FixedRateLiquidityPluginClient, PluginError, DEFAULT_FEE_BPS,
    PLUGIN_NAME,
};
use router_core::{RouterCore, RouterCoreClient};
use router_registry::{RouterRegistry, RouterRegistryClient};

// ── Shared test fixture ───────────────────────────────────────────────────────

struct Suite<'a> {
    env: Env,
    admin: Address,
    plugin_id: Address,
    route_name: String,
    core: RouterCoreClient<'a>,
    registry: RouterRegistryClient<'a>,
    plugin: FixedRateLiquidityPluginClient<'a>,
}

fn setup() -> Suite<'static> {
    let env = Env::default();
    env.mock_all_auths();
    env.ledger().with_mut(|l| l.timestamp = 1000);

    let admin = Address::generate(&env);

    let core_id = env.register_contract(None, RouterCore);
    let registry_id = env.register_contract(None, RouterRegistry);
    let plugin_id = env.register_contract(None, FixedRateLiquidityPlugin);

    let core = RouterCoreClient::new(&env, &core_id);
    let registry = RouterRegistryClient::new(&env, &registry_id);
    let plugin = FixedRateLiquidityPluginClient::new(&env, &plugin_id);

    core.initialize(&admin);
    registry.initialize(&admin);

    let route_name = String::from_str(&env, PLUGIN_NAME);

    Suite {
        env,
        admin,
        plugin_id,
        route_name,
        core,
        registry,
        plugin,
    }
}

// ── Test 1: Registration in router-registry ──────────────────────────────────
//
// Verifies the plugin can be registered under its well-known name + a version,
// then looked up again via get_latest and reverse address lookup.

#[test]
fn test_registry_registration_and_lookup() {
    let s = setup();

    s.registry
        .register(&s.admin, &s.route_name, &s.plugin_id, &1);

    let entry = s.registry.get_latest(&s.route_name);
    assert_eq!(entry.address, s.plugin_id);
    assert_eq!(entry.name, s.route_name);
    assert_eq!(entry.version, 1);
    assert!(!entry.deprecated);

    // Reverse lookup by address must return the same entry.
    let by_address = s.registry.get_entry_by_address(&s.plugin_id).unwrap();
    assert_eq!(by_address.address, s.plugin_id);
    assert_eq!(by_address.version, 1);

    // The plugin reports its own name/version, matching the registration.
    assert_eq!(s.plugin.name(), String::from_str(&s.env, PLUGIN_NAME));
    assert_eq!(s.plugin.version(), 1);
}

// ── Test 2: Routing via router-core ──────────────────────────────────────────
//
// Verifies a router-core route can point at the plugin and resolve() returns
// the plugin address so callers can invoke it.

#[test]
fn test_route_registration_and_resolution() {
    let s = setup();

    s.core
        .register_route(&s.admin, &s.route_name, &s.plugin_id, &None);

    let resolved = s.core.resolve(&s.route_name);
    assert_eq!(resolved, s.plugin_id);
    assert_eq!(s.core.total_routed(), 1);

    // The route entry points at the plugin address.
    let entry = s.core.get_route(&s.route_name).unwrap();
    assert_eq!(entry.address, s.plugin_id);
}

// ── Test 3: Full pipeline — register → route → quote → swap ─────────────────
//
// End-to-end per docs/plugin-system.md: registration + routing, then quotes
// and an executed swap invoked through the resolved plugin address.

#[test]
fn test_full_pipeline_quote_and_swap() {
    let s = setup();
    let caller = Address::generate(&s.env);
    let token_in = Address::generate(&s.env);
    let token_out = Address::generate(&s.env);

    s.registry
        .register(&s.admin, &s.route_name, &s.plugin_id, &1);
    s.core
        .register_route(&s.admin, &s.route_name, &s.plugin_id, &None);

    // Route through router-core to the plugin address.
    let resolved = s.core.resolve(&s.route_name);
    assert_eq!(resolved, s.plugin_id);
    let routed = FixedRateLiquidityPluginClient::new(&s.env, &resolved);

    // Quote: amount_out = 10_000 * (10_000 - 30) / 10_000 = 9_970.
    let quote = routed.get_quote(&token_in, &token_out, &10_000);
    assert_eq!(quote, 9_970);

    let quote_event_emitted = s.env.events().all().iter().any(|e| {
        e.1.get(0)
            .and_then(|v| Symbol::try_from_val(&s.env, &v).ok())
            .map(|sym| sym == Symbol::new(&s.env, "quote_generated"))
            .unwrap_or(false)
    });
    assert!(quote_event_emitted, "quote_generated event must be emitted");

    // Swap: same fee model, no slippage tolerated beyond the exact output.
    let amount_out = routed.execute_swap(&caller, &token_in, &token_out, &10_000, &9_970);
    assert_eq!(amount_out, 9_970);
    assert_eq!(routed.swap_count(), 1);

    let swap_event_emitted = s.env.events().all().iter().any(|e| {
        e.1.get(0)
            .and_then(|v| Symbol::try_from_val(&s.env, &v).ok())
            .map(|sym| sym == Symbol::new(&s.env, "swap_executed"))
            .unwrap_or(false)
    });
    assert!(swap_event_emitted, "swap_executed event must be emitted");
}

// ── Test 4: Slippage protection ─────────────────────────────────────────────
//
// Verifies execute_swap rejects trades whose output would fall below the
// caller's min_amount_out, and that no state is mutated on failure.

#[test]
fn test_swap_slippage_rejection() {
    let s = setup();
    let caller = Address::generate(&s.env);
    let token_in = Address::generate(&s.env);
    let token_out = Address::generate(&s.env);

    s.registry
        .register(&s.admin, &s.route_name, &s.plugin_id, &1);
    s.core
        .register_route(&s.admin, &s.route_name, &s.plugin_id, &None);

    let resolved = s.core.resolve(&s.route_name);
    let routed = FixedRateLiquidityPluginClient::new(&s.env, &resolved);

    // Output is 9_970; demanding 9_971 must be rejected.
    let result = routed.try_execute_swap(&caller, &token_in, &token_out, &10_000, &9_971);
    assert_eq!(result, Err(Ok(PluginError::InsufficientOutput)));

    // No swap was recorded.
    assert_eq!(routed.swap_count(), 0);
}

// ── Test 5: Admin fee control through the routed plugin ──────────────────────
//
// Verifies the expected fee can be changed by the admin and takes effect in
// subsequent quotes. Also verifies the DEFAULT_FEE_BPS constant exported by
// the crate matches the default used on-chain.

#[test]
fn test_admin_adjusts_fee_through_route() {
    let s = setup();

    s.core
        .register_route(&s.admin, &s.route_name, &s.plugin_id, &None);
    let resolved = s.core.resolve(&s.route_name);
    let routed = FixedRateLiquidityPluginClient::new(&s.env, &resolved);

    // Admin operations require the plugin to be initialized.
    s.plugin.initialize(&s.admin);

    let token_in = Address::generate(&s.env);
    let token_out = Address::generate(&s.env);

    // Default fee applies when no custom fee is set.
    assert_eq!(routed.fee_bps(), DEFAULT_FEE_BPS);
    assert_eq!(routed.get_quote(&token_in, &token_out, &1_000), 997);

    // Admin raises the fee to 100 bps (1%) → output drops.
    routed.set_fee(&s.admin, &100);
    assert_eq!(routed.fee_bps(), 100);
    assert_eq!(routed.get_quote(&token_in, &token_out, &1_000), 990);

    // A non-admin cannot change the fee.
    let attacker = Address::generate(&s.env);
    let result = routed.try_set_fee(&attacker, &0);
    assert_eq!(result, Err(Ok(PluginError::Unauthorized)));
}

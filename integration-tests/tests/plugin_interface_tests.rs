/// Integration tests for the canonical `LiquidityPlugin` interface
/// (contracts/router-plugin-interface).
///
/// Where `liquidity_plugin_tests.rs` exercises one concrete plugin through its
/// own generated client, these tests exercise the *interface*: every call goes
/// through `LiquidityPluginClient`, the client generated from the trait, which
/// knows nothing about the contract behind the address it is given.
///
/// 1. **Routing** — a plugin implementing the interface is registered in
///    router-registry, routed through router-core, and invoked through the
///    generic client at the resolved address.
/// 2. **Compatibility** — the pre-existing `liquidity-plugin-example-fixed-rate`
///    contract is driven through the same generic client, confirming the trait
///    describes the plugin ABI the repository already ships.
/// 3. **Errors** — interface errors survive the round trip through routing.
///
/// These tests run entirely in the Soroban test environment — no testnet
/// required. Run with:
///   cargo test -p integration-tests --test plugin_interface_tests
extern crate std;

use soroban_sdk::{testutils::Address as _, Address, Env, String};

use liquidity_plugin_example_fixed_rate::{FixedRateLiquidityPlugin, PLUGIN_NAME};
use router_core::{RouterCore, RouterCoreClient};
use router_plugin_interface::{
    LiquidityPluginClient, NoopLiquidityPlugin, PluginError, NOOP_PLUGIN_NAME, NOOP_PLUGIN_VERSION,
};
use router_registry::{RouterRegistry, RouterRegistryClient};

// ── Shared test fixture ───────────────────────────────────────────────────────

struct Suite<'a> {
    env: Env,
    admin: Address,
    core: RouterCoreClient<'a>,
    registry: RouterRegistryClient<'a>,
}

fn setup() -> Suite<'static> {
    let env = Env::default();
    env.mock_all_auths();

    let admin = Address::generate(&env);

    let core = RouterCoreClient::new(&env, &env.register_contract(None, RouterCore));
    let registry = RouterRegistryClient::new(&env, &env.register_contract(None, RouterRegistry));

    core.initialize(&admin);
    registry.initialize(&admin);

    Suite {
        env,
        admin,
        core,
        registry,
    }
}

// ── Test 1: Full lifecycle through the generic interface client ──────────────
//
// Registers an interface implementation under its well-known name, points a
// router-core route at it, then quotes and swaps through the resolved address
// using LiquidityPluginClient. This is the call path a router has when it only
// knows that the contract satisfies the interface.

#[test]
fn test_interface_plugin_routed_through_core() {
    let s = setup();
    let plugin_id = s.env.register_contract(None, NoopLiquidityPlugin);
    let route_name = String::from_str(&s.env, NOOP_PLUGIN_NAME);

    s.registry.register(&s.admin, &route_name, &plugin_id, &1);
    s.core
        .register_route(&s.admin, &route_name, &plugin_id, &None);

    let resolved = s.core.resolve(&route_name);
    assert_eq!(resolved, plugin_id);

    // Everything below goes through the trait-generated client.
    let plugin = LiquidityPluginClient::new(&s.env, &resolved);

    assert_eq!(plugin.name(), route_name);
    assert_eq!(plugin.version(), NOOP_PLUGIN_VERSION);

    let token_in = Address::generate(&s.env);
    let token_out = Address::generate(&s.env);
    let caller = Address::generate(&s.env);

    // The no-op plugin quotes one-for-one.
    assert_eq!(plugin.get_quote(&token_in, &token_out, &10_000), 10_000);

    let amount_out = plugin.execute_swap(&caller, &token_in, &token_out, &10_000, &10_000);
    assert_eq!(amount_out, 10_000);
}

// ── Test 2: The interface matches the existing reference plugin ──────────────
//
// liquidity-plugin-example-fixed-rate was written before the trait existed. If
// the trait is a faithful description of the plugin ABI, the generic client
// must be able to drive that contract too — without it implementing the trait
// in Rust. This is what makes the interface authoritative rather than
// aspirational.

#[test]
fn test_generic_client_drives_existing_fixed_rate_plugin() {
    let s = setup();
    let plugin_id = s.env.register_contract(None, FixedRateLiquidityPlugin);
    let route_name = String::from_str(&s.env, PLUGIN_NAME);

    s.registry.register(&s.admin, &route_name, &plugin_id, &1);
    s.core
        .register_route(&s.admin, &route_name, &plugin_id, &None);

    let resolved = s.core.resolve(&route_name);
    let plugin = LiquidityPluginClient::new(&s.env, &resolved);

    assert_eq!(plugin.name(), route_name);
    assert_eq!(plugin.version(), 1);

    let token_in = Address::generate(&s.env);
    let token_out = Address::generate(&s.env);
    let caller = Address::generate(&s.env);

    // Fixed-rate applies its 30 bps fee: 10_000 → 9_970.
    assert_eq!(plugin.get_quote(&token_in, &token_out, &10_000), 9_970);
    assert_eq!(
        plugin.execute_swap(&caller, &token_in, &token_out, &10_000, &9_970),
        9_970
    );
}

// ── Test 3: Interface errors survive routing ─────────────────────────────────
//
// The error codes declared on PluginError are part of the interface, so a
// caller holding only the generic client must be able to match on them after
// resolving a route.

#[test]
fn test_interface_errors_propagate_through_route() {
    let s = setup();
    let plugin_id = s.env.register_contract(None, NoopLiquidityPlugin);
    let route_name = String::from_str(&s.env, NOOP_PLUGIN_NAME);

    s.core
        .register_route(&s.admin, &route_name, &plugin_id, &None);

    let resolved = s.core.resolve(&route_name);
    let plugin = LiquidityPluginClient::new(&s.env, &resolved);

    let token = Address::generate(&s.env);
    let token_out = Address::generate(&s.env);
    let caller = Address::generate(&s.env);

    assert_eq!(
        plugin.try_get_quote(&token, &token, &1_000),
        Err(Ok(PluginError::SameToken))
    );
    assert_eq!(
        plugin.try_get_quote(&token, &token_out, &0),
        Err(Ok(PluginError::InvalidAmount))
    );
    // Output is 1_000; demanding 1_001 must be rejected.
    assert_eq!(
        plugin.try_execute_swap(&caller, &token, &token_out, &1_000, &1_001),
        Err(Ok(PluginError::InsufficientOutput))
    );
}

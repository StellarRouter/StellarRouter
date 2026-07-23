/// End-to-end integration tests for the router-core → router-quote → router-execution pipeline.
///
/// These tests run entirely in the Soroban test environment — no testnet required.
/// They exercise the composition that was previously untested: get a quote for a
/// resolved route, then invoke router-execution.execute against the resolved address.
///
/// Acceptance criteria covered:
///
/// 1. **Happy path** — register a route in router-core, resolve it, get a quote via
///    router-quote for that route/target, invoke router-execution.execute against the
///    resolved address, assert the full chain succeeds and all counters/events are correct.
///
/// 2. **Paused-mid-flow** — repeat with the route paused in router-core after the quote
///    is obtained, asserting that downstream resolve/execution calls see consistent state
///    (resolve returns RoutePaused, execution is not attempted against a stale address).
///
/// Run with:
///   cargo test --test quote_execution_tests
extern crate std;

use soroban_sdk::{
    contract, contractimpl,
    testutils::Address as _,
    Address, Env, String, Symbol, TryFromVal,
};

use router_core::{RouterCore, RouterCoreClient, RouterError};
use router_execution::{ExecutionError, ExecutionRequest, RouterExecution, RouterExecutionClient};
use router_quote::{QuoteRequest, RouterQuote, RouterQuoteClient};

// ── Minimal mock target contract ──────────────────────────────────────────────
//
// router-execution.execute calls try_invoke_contract internally.  We need a real
// registered contract at the resolved address so the call succeeds.
// This contract exposes a single `ping` entry point that returns a Symbol.

#[contract]
pub struct MockTarget;

#[contractimpl]
impl MockTarget {
    pub fn ping(env: Env) -> Symbol {
        Symbol::new(&env, "pong")
    }
}

// ── Shared test fixture ───────────────────────────────────────────────────────

struct Suite<'a> {
    env: Env,
    admin: Address,
    core: RouterCoreClient<'a>,
    quote: RouterQuoteClient<'a>,
    execution: RouterExecutionClient<'a>,
    /// Address of the MockTarget contract — this is what router-core routes point to.
    target_addr: Address,
}

fn setup() -> Suite<'static> {
    let env = Env::default();
    env.mock_all_auths();
    env.ledger().with_mut(|l| l.timestamp = 1000);

    let admin = Address::generate(&env);

    let core_id = env.register_contract(None, RouterCore);
    let quote_id = env.register_contract(None, RouterQuote);
    let execution_id = env.register_contract(None, RouterExecution);
    let target_addr = env.register_contract(None, MockTarget);

    let core = RouterCoreClient::new(&env, &core_id);
    let quote = RouterQuoteClient::new(&env, &quote_id);
    let execution = RouterExecutionClient::new(&env, &execution_id);

    core.initialize(&admin);
    quote.initialize(&admin, &100); // 1% default fee
    execution.initialize(
        &admin,
        &2,   // max_retries cap = 2
        &0,   // backoff_base_ms = 0 (no wall-clock delay in tests)
        &100, // backoff_multiplier = 1x (no escalation)
    );

    Suite { env, admin, core, quote, execution, target_addr }
}

// ── Test 1: Full happy-path pipeline ─────────────────────────────────────────
//
// 1. Register a route in router-core pointing to MockTarget.
// 2. Resolve the route — confirm the right address comes back.
// 3. Get a quote via router-quote for that route/amount.
// 4. Invoke router-execution.execute against the resolved address.
// 5. Assert all results, counters, and events are correct.

#[test]
fn test_full_quote_execution_pipeline() {
    let s = setup();
    let route_name = String::from_str(&s.env, "oracle");
    let caller = Address::generate(&s.env);

    // Step 1 — register route
    s.core.register_route(&s.admin, &route_name, &s.target_addr, &None);

    // Step 2 — resolve
    let resolved_addr = s.core.resolve(&route_name);
    assert_eq!(resolved_addr, s.target_addr, "resolve must return target_addr");
    assert_eq!(s.core.total_routed(), 1);

    // Step 3 — quote (30 bps = 0.3% fee)
    s.quote.set_route_fee(&s.admin, &route_name, &30);
    let quote_resp = s.quote.get_quote(&QuoteRequest {
        route: route_name.clone(),
        token_in: Address::generate(&s.env),
        token_out: Address::generate(&s.env),
        amount_in: 10_000,
    });

    // fee_amount = 10_000 * 30 / 10_000 = 3
    assert_eq!(quote_resp.fee_bps, 30);
    assert_eq!(quote_resp.fee_amount, 3);
    assert_eq!(quote_resp.amount_out, 9_997);
    assert_eq!(quote_resp.price_impact_bps, 3); // 3 * 10_000 / 10_000 = 3
    assert_eq!(quote_resp.route, route_name);

    // Verify quote_calculated event was emitted by the quote contract.
    let quote_event_emitted = s.env.events().all().iter().any(|e| {
        e.1.get(0)
            .and_then(|v| Symbol::try_from_val(&s.env, &v).ok())
            .map(|sym| sym == Symbol::new(&s.env, "quote_calculated"))
            .unwrap_or(false)
    });
    assert!(quote_event_emitted, "quote_calculated event must be emitted");

    // Step 4 — execute
    let exec_result = s.execution.execute(
        &caller,
        &ExecutionRequest {
            target: resolved_addr.clone(),
            function: Symbol::new(&s.env, "ping"),
            simulate_first: false,
            max_retries: 0,
        },
    );

    // Step 5 — assert execution outcome
    assert!(exec_result.success, "execution must succeed");
    assert_eq!(exec_result.attempts, 1, "must succeed on first attempt");
    assert!(!exec_result.simulated);
    assert_eq!(exec_result.target, s.target_addr);

    let (total_execs, total_errors) = s.execution.stats();
    assert_eq!(total_execs, 1);
    assert_eq!(total_errors, 0);

    let history = s.execution.get_execution_history(&10);
    assert_eq!(history.len(), 1);
    let record = history.get(0).unwrap();
    assert!(record.success);
    assert_eq!(record.function, Symbol::new(&s.env, "ping"));

    let exec_event_emitted = s.env.events().all().iter().any(|e| {
        e.1.get(0)
            .and_then(|v| Symbol::try_from_val(&s.env, &v).ok())
            .map(|sym| sym == Symbol::new(&s.env, "execution_result"))
            .unwrap_or(false)
    });
    assert!(exec_event_emitted, "execution_result event must be emitted");

    // total_routed was only incremented once (by the single resolve above).
    assert_eq!(s.core.total_routed(), 1);
}

// ── Test 2: Pipeline with simulate_first=true ─────────────────────────────────
//
// Same happy-path but verifies the simulation gate passes cleanly when the
// target is a real registered contract.

#[test]
fn test_full_pipeline_with_simulation() {
    let s = setup();
    let route_name = String::from_str(&s.env, "swap");
    let caller = Address::generate(&s.env);

    s.core.register_route(&s.admin, &route_name, &s.target_addr, &None);

    let resolved = s.core.resolve(&route_name);
    assert_eq!(resolved, s.target_addr);

    // Default fee = 100 bps (1%). amount_in=1_000 → fee=10, amount_out=990.
    let resp = s.quote.get_quote(&QuoteRequest {
        route: route_name.clone(),
        token_in: Address::generate(&s.env),
        token_out: Address::generate(&s.env),
        amount_in: 1_000,
    });
    assert_eq!(resp.fee_bps, 100);
    assert_eq!(resp.amount_out, 990);

    let exec_result = s.execution.execute(
        &caller,
        &ExecutionRequest {
            target: resolved,
            function: Symbol::new(&s.env, "ping"),
            simulate_first: true,
            max_retries: 0,
        },
    );

    assert!(exec_result.success);
    assert!(exec_result.simulated, "simulated flag must be true");
    assert_eq!(exec_result.attempts, 1);
}

// ── Test 3: Best quote selection then execute ─────────────────────────────────
//
// Registers two routes with different fees, selects the best quote, then
// resolves and executes against the winning route's address.

#[test]
fn test_best_quote_then_execute() {
    let s = setup();

    let route_a = String::from_str(&s.env, "route-a");
    let route_b = String::from_str(&s.env, "route-b");

    s.core.register_route(&s.admin, &route_a, &s.target_addr, &None);
    s.core.register_route(&s.admin, &route_b, &s.target_addr, &None);

    // route-a: 50 bps, route-b: 20 bps → route-b has lower fee → more amount_out
    s.quote.set_route_fee(&s.admin, &route_a, &50);
    s.quote.set_route_fee(&s.admin, &route_b, &20);

    let token_in = Address::generate(&s.env);
    let token_out = Address::generate(&s.env);

    let requests = soroban_sdk::vec![
        &s.env,
        QuoteRequest {
            route: route_a.clone(),
            token_in: token_in.clone(),
            token_out: token_out.clone(),
            amount_in: 10_000,
        },
        QuoteRequest {
            route: route_b.clone(),
            token_in: token_in.clone(),
            token_out: token_out.clone(),
            amount_in: 10_000,
        },
    ];

    let best = s.quote.get_best_quote(&requests);

    // route-b: fee_amount = 10_000 * 20 / 10_000 = 2 → amount_out = 9_998
    assert_eq!(best.route, route_b, "route-b must win as best quote");
    assert_eq!(best.fee_bps, 20);
    assert_eq!(best.amount_out, 9_998);

    let resolved = s.core.resolve(&best.route);
    assert_eq!(resolved, s.target_addr);

    let caller = Address::generate(&s.env);
    let exec_result = s.execution.execute(
        &caller,
        &ExecutionRequest {
            target: resolved,
            function: Symbol::new(&s.env, "ping"),
            simulate_first: false,
            max_retries: 0,
        },
    );
    assert!(exec_result.success);
    assert_eq!(exec_result.attempts, 1);
}

// ── Test 4: Route paused mid-flow — downstream sees consistent state ──────────
//
// Acceptance criterion 2: run the full pipeline once (succeeds), then pause
// the route in router-core and repeat. Assert:
//   - resolve() returns RouterError::RoutePaused.
//   - total_routed does NOT increment.
//   - router-quote is stateless w.r.t. core pause — quote still computes.
//   - After unpause, the full flow works again.

#[test]
fn test_route_paused_mid_flow_blocks_downstream() {
    let s = setup();
    let route_name = String::from_str(&s.env, "oracle");
    let caller = Address::generate(&s.env);

    s.core.register_route(&s.admin, &route_name, &s.target_addr, &None);
    s.quote.set_route_fee(&s.admin, &route_name, &50);

    // ── Pre-pause: full flow succeeds ─────────────────────────────────────
    let resolved_before = s.core.resolve(&route_name);
    assert_eq!(resolved_before, s.target_addr);
    assert_eq!(s.core.total_routed(), 1);

    // fee_amount = 5_000 * 50 / 10_000 = 25 → amount_out = 4_975
    let quote_before = s.quote.get_quote(&QuoteRequest {
        route: route_name.clone(),
        token_in: Address::generate(&s.env),
        token_out: Address::generate(&s.env),
        amount_in: 5_000,
    });
    assert_eq!(quote_before.fee_bps, 50);
    assert_eq!(quote_before.amount_out, 4_975);

    let exec_before = s.execution.execute(
        &caller,
        &ExecutionRequest {
            target: resolved_before.clone(),
            function: Symbol::new(&s.env, "ping"),
            simulate_first: false,
            max_retries: 0,
        },
    );
    assert!(exec_before.success, "execution before pause must succeed");

    // ── Pause the route ───────────────────────────────────────────────────
    s.core.set_route_paused(&s.admin, &route_name, &true);

    // resolve() must now return RoutePaused.
    assert_eq!(
        s.core.try_resolve(&route_name),
        Err(Ok(RouterError::RoutePaused)),
        "resolve must return RoutePaused when route is paused"
    );

    // total_routed must NOT have incremented on the failed resolve.
    assert_eq!(
        s.core.total_routed(),
        1,
        "total_routed must not increment on a paused resolve"
    );

    // router-quote is independent of core's pause state: quote still computes
    // correctly. The caller's responsibility is to gate on resolve() first.
    let quote_while_paused = s.quote.get_quote(&QuoteRequest {
        route: route_name.clone(),
        token_in: Address::generate(&s.env),
        token_out: Address::generate(&s.env),
        amount_in: 5_000,
    });
    assert_eq!(
        quote_while_paused.fee_bps, 50,
        "quote fee is unchanged while route is paused in core"
    );
    assert_eq!(quote_while_paused.amount_out, 4_975);

    // Direct execution to the target address itself still works — core's pause
    // guards route *discovery* (resolve), not the underlying contract call.
    // A caller that bypasses resolve() with a cached address can still invoke
    // the target, but it will have stale routing information. The test
    // documents this boundary explicitly.
    let exec_stale = s.execution.try_execute(
        &caller,
        &ExecutionRequest {
            target: resolved_before.clone(),
            function: Symbol::new(&s.env, "ping"),
            simulate_first: false,
            max_retries: 0,
        },
    );
    assert!(
        exec_stale.is_ok(),
        "direct execution to target still works — core pause guards resolve(), not the target"
    );

    // ── Unpause and verify the full flow is restored ──────────────────────
    s.core.set_route_paused(&s.admin, &route_name, &false);

    let resolved_after = s.core.resolve(&route_name);
    assert_eq!(resolved_after, s.target_addr, "resolve must work after unpause");
    assert_eq!(
        s.core.total_routed(),
        2,
        "total_routed increments on post-unpause resolve"
    );

    let quote_after = s.quote.get_quote(&QuoteRequest {
        route: route_name.clone(),
        token_in: Address::generate(&s.env),
        token_out: Address::generate(&s.env),
        amount_in: 5_000,
    });
    assert_eq!(quote_after.amount_out, 4_975, "quote is identical after unpause");

    let exec_after = s.execution.execute(
        &caller,
        &ExecutionRequest {
            target: resolved_after,
            function: Symbol::new(&s.env, "ping"),
            simulate_first: false,
            max_retries: 0,
        },
    );
    assert!(exec_after.success, "execution after unpause must succeed");
}

// ── Test 5: Global router pause mid-flow ─────────────────────────────────────
//
// Same structure as Test 4 but pauses the entire router instead of a single
// route. Verifies RouterPaused propagates from resolve(), and that quote and
// execution are independently unaffected by the global pause flag.

#[test]
fn test_global_router_pause_mid_flow() {
    let s = setup();
    let route_name = String::from_str(&s.env, "vault");
    let caller = Address::generate(&s.env);

    s.core.register_route(&s.admin, &route_name, &s.target_addr, &None);
    s.quote.set_route_fee(&s.admin, &route_name, &30);

    // Pre-pause: resolve + quote + execute all succeed.
    let addr = s.core.resolve(&route_name);
    assert_eq!(addr, s.target_addr);

    let q = s.quote.get_quote(&QuoteRequest {
        route: route_name.clone(),
        token_in: Address::generate(&s.env),
        token_out: Address::generate(&s.env),
        amount_in: 20_000,
    });
    // fee_amount = 20_000 * 30 / 10_000 = 60 → amount_out = 19_940
    assert_eq!(q.fee_bps, 30);
    assert_eq!(q.amount_out, 19_940);

    let er = s.execution.execute(
        &caller,
        &ExecutionRequest {
            target: addr,
            function: Symbol::new(&s.env, "ping"),
            simulate_first: false,
            max_retries: 0,
        },
    );
    assert!(er.success);
    let routed_before = s.core.total_routed();
    assert_eq!(routed_before, 1);

    // ── Pause entire router ───────────────────────────────────────────────
    s.core.set_paused(&s.admin, &true);

    // resolve() must return RouterPaused for any named route.
    assert_eq!(
        s.core.try_resolve(&route_name),
        Err(Ok(RouterError::RouterPaused))
    );
    assert_eq!(s.core.total_routed(), routed_before, "counter must not change on paused resolve");

    // get_best_route also respects the global pause.
    let candidates = soroban_sdk::vec![&s.env, route_name.clone()];
    assert_eq!(
        s.core.try_get_best_route(&candidates, &0, &None),
        Err(Ok(RouterError::RouterPaused))
    );

    // Quote contract is independent of core's global pause.
    let q2 = s.quote.get_quote(&QuoteRequest {
        route: route_name.clone(),
        token_in: Address::generate(&s.env),
        token_out: Address::generate(&s.env),
        amount_in: 20_000,
    });
    assert_eq!(q2.fee_bps, 30, "quote unaffected by global core pause");
    assert_eq!(q2.amount_out, 19_940);

    // ── Unpause and verify everything works again ─────────────────────────
    s.core.set_paused(&s.admin, &false);

    let addr2 = s.core.resolve(&route_name);
    assert_eq!(addr2, s.target_addr);
    assert_eq!(s.core.total_routed(), routed_before + 1);

    let er2 = s.execution.execute(
        &caller,
        &ExecutionRequest {
            target: addr2,
            function: Symbol::new(&s.env, "ping"),
            simulate_first: false,
            max_retries: 0,
        },
    );
    assert!(er2.success);
}

// ── Test 6: compare_quotes filtering then execute ────────────────────────────
//
// Registers three routes with different fee tiers, calls compare_quotes with
// a threshold that eliminates the most expensive route, then resolves and
// executes against the best surviving quote.

#[test]
fn test_compare_quotes_filter_then_execute() {
    let s = setup();
    let caller = Address::generate(&s.env);

    let route_cheap = String::from_str(&s.env, "cheap");
    let route_mid = String::from_str(&s.env, "mid");
    let route_expensive = String::from_str(&s.env, "expensive");

    s.core.register_route(&s.admin, &route_cheap, &s.target_addr, &None);
    s.core.register_route(&s.admin, &route_mid, &s.target_addr, &None);
    s.core.register_route(&s.admin, &route_expensive, &s.target_addr, &None);

    // cheap: 10 bps, mid: 50 bps, expensive: 200 bps
    s.quote.set_route_fee(&s.admin, &route_cheap, &10);
    s.quote.set_route_fee(&s.admin, &route_mid, &50);
    s.quote.set_route_fee(&s.admin, &route_expensive, &200);

    let tok_in = Address::generate(&s.env);
    let tok_out = Address::generate(&s.env);

    let requests = soroban_sdk::vec![
        &s.env,
        QuoteRequest {
            route: route_cheap.clone(),
            token_in: tok_in.clone(),
            token_out: tok_out.clone(),
            amount_in: 10_000,
        },
        QuoteRequest {
            route: route_mid.clone(),
            token_in: tok_in.clone(),
            token_out: tok_out.clone(),
            amount_in: 10_000,
        },
        QuoteRequest {
            route: route_expensive.clone(),
            token_in: tok_in.clone(),
            token_out: tok_out.clone(),
            amount_in: 10_000,
        },
    ];

    // Threshold 100 bps: expensive (200 bps impact) is filtered out.
    let filtered = s.quote.compare_quotes(&requests, &100_i128);
    assert_eq!(filtered.len(), 2, "cheap and mid must survive; expensive must not");

    // compare_quotes returns survivors sorted by amount_out descending.
    let winner = filtered.get(0).unwrap();
    assert_eq!(winner.route, route_cheap, "cheap route must be first (best amount_out)");
    assert_eq!(winner.fee_bps, 10);
    // cheap: fee_amount = 10_000 * 10 / 10_000 = 1 → amount_out = 9_999
    assert_eq!(winner.amount_out, 9_999);

    let resolved = s.core.resolve(&winner.route);
    assert_eq!(resolved, s.target_addr);

    let er = s.execution.execute(
        &caller,
        &ExecutionRequest {
            target: resolved,
            function: Symbol::new(&s.env, "ping"),
            simulate_first: false,
            max_retries: 0,
        },
    );
    assert!(er.success);
    assert_eq!(er.attempts, 1);
}

// ── Test 7: Execution against invalid (non-contract) target ──────────────────
//
// Registers a route pointing to a plain address with no contract code.
// Verifies that:
//   - resolve() and get_quote() both succeed (they are unaware of target validity).
//   - execute() fails with ContractRejected after exhausting retries.
//   - router-core routing state is unchanged after the failed execution.

#[test]
fn test_execution_fails_gracefully_on_invalid_target() {
    let s = setup();
    let route_name = String::from_str(&s.env, "broken-route");
    let caller = Address::generate(&s.env);

    let non_contract_addr = Address::generate(&s.env);
    s.core.register_route(&s.admin, &route_name, &non_contract_addr, &None);
    s.quote.set_route_fee(&s.admin, &route_name, &50);

    // Resolve succeeds — routing layer has no knowledge the target is invalid.
    let resolved = s.core.resolve(&route_name);
    assert_eq!(resolved, non_contract_addr);

    // Quote succeeds — also stateless w.r.t. target validity.
    let q = s.quote.get_quote(&QuoteRequest {
        route: route_name.clone(),
        token_in: Address::generate(&s.env),
        token_out: Address::generate(&s.env),
        amount_in: 1_000,
    });
    assert_eq!(q.fee_bps, 50);

    // Execute must fail with ContractRejected.
    let exec_result = s.execution.try_execute(
        &caller,
        &ExecutionRequest {
            target: resolved,
            function: Symbol::new(&s.env, "ping"),
            simulate_first: false,
            max_retries: 0,
        },
    );
    assert_eq!(
        exec_result,
        Err(Ok(ExecutionError::ContractRejected)),
        "execution against a non-contract address must return ContractRejected"
    );

    // router-core route entry is unchanged after the failed execution.
    let entry = s.core.get_route(&route_name);
    assert!(entry.is_some(), "route entry must still exist");
    assert_eq!(entry.unwrap().address, non_contract_addr);
}

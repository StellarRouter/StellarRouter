# StellarRouter Integration Testing Guide

This guide covers running and writing integration tests for the `stellar-router` project.

## Overview

`stellar-router` includes two types of integration test suites:

1. **Testnet Integration Tests (`integration-tests/tests/integration_tests.rs`)**:
   Run against the live Stellar testnet using `stellar-cli` and Friendbot funding.
   These verify live contract deployments, real network RPC calls, and testnet interactions.

2. **In-Memory Integration Tests**:
   Run entirely in the Soroban local test environment (`Env`) without network access.
   - `cross_contract_tests.rs` — Cross-contract interaction between router-core, registry, access, middleware, and multicall.
   - `quote_execution_tests.rs` — End-to-end routing, quote calculation, and execution pipeline.
   - `liquidity_plugin_tests.rs` — Liquidity plugin lifecycle, registration, routing, quotes, and swaps.
   - `failure_scenarios.rs` — Error handling, pause controls, rate limiting, and timelock edge cases.

---

## Prerequisites

Before running testnet integration tests, ensure you have:

1. **Rust (stable)** with `wasm32-unknown-unknown` target:
   ```bash
   rustup target add wasm32-unknown-unknown
   ```

2. **Stellar CLI**:
   ```bash
   cargo install --locked stellar-cli
   ```

3. **WASM Contract Artifacts**:
   Build all router contract WASM binaries:
   ```bash
   cargo build --target wasm32-unknown-unknown --release
   ```

---

## Running Integration Tests

### Method 1: Using the Automated Test Script

The recommended way to run testnet integration tests is via the provided script, which automatically checks prerequisites and builds WASM artifacts if needed:

```bash
./scripts/run-integration-tests.sh
```

**Script Options:**
```bash
./scripts/run-integration-tests.sh --filter full_flow          # Filter tests by pattern
./scripts/run-integration-tests.sh --filter test_full_router_core_flow # Run a specific test
./scripts/run-integration-tests.sh --quiet                   # Suppress stdout
./scripts/run-integration-tests.sh --parallel                # Run tests in parallel
```

### Method 2: Running via Cargo Directly

To run testnet integration tests directly using `cargo test`:

```bash
# Run all testnet integration tests
cargo test --test integration_tests -- --ignored --test-threads=1 --nocapture

# Run a specific testnet integration test
cargo test --test integration_tests test_full_router_core_flow -- --ignored --nocapture
```

> **Note:** Testnet integration tests are marked `#[ignore]` by default so they do not run during standard `cargo test`. Using `--test-threads=1` is recommended to avoid hitting testnet rate limits.

### Method 3: Running In-Memory Integration Tests

In-memory integration tests run quickly without testnet access:

```bash
cargo test --test cross_contract_tests
cargo test --test quote_execution_tests
cargo test --test liquidity_plugin_tests
cargo test --test failure_scenarios
```

---

## Test Directory Structure

```
integration-tests/
├── Cargo.toml
└── tests/
    ├── integration_tests.rs       # Main testnet test suite
    ├── cross_contract_tests.rs    # Core + Registry + Access + Middleware + Multicall
    ├── quote_execution_tests.rs   # Core + Quote + Execution pipeline
    ├── liquidity_plugin_tests.rs  # Plugin lifecycle and swap execution
    ├── failure_scenarios.rs       # Error handling and edge cases
    ├── integration/               # Testnet helpers & scenarios
    │   ├── testnet_setup.rs       # Account generation, funding, CLI invocation
    │   ├── full_flow_test.rs      # Happy-path testnet workflows
    │   └── failure_scenarios.rs   # Error-path testnet workflows
    └── README.md                  # Test suite overview
```

---

## Further Documentation

For details on test utilities, fixture setup (`TestAccount`, `DeployedContract`, `TestFixture`), and specific scenario descriptions, see [`integration-tests/tests/README.md`](integration-tests/tests/README.md).

# Integration Tests

This directory contains integration tests for the stellar-router project that run against Stellar testnet.

## Quick Start

### 1. Prerequisites

```bash
# Install Stellar CLI
cargo install --locked stellar-cli

# Build WASM contracts
cargo build --target wasm32-unknown-unknown --release
```

### 2. Run Tests

```bash
# Run all integration tests
cargo test --test integration_tests -- --ignored --test-threads=1 --nocapture

# Run specific test
cargo test --test integration_tests test_full_router_core_flow -- --ignored --nocapture

# Run quick validation tests
cargo test --test integration_tests quick_tests -- --ignored --nocapture
```

## Test Structure

```
integration-tests/
├── Cargo.toml                    # Test package configuration
└── tests/
    ├── integration_tests.rs      # Main test file
    ├── integration/
    │   ├── mod.rs                # Module declarations
    │   ├── testnet_setup.rs      # Testnet utilities and fixtures
    │   ├── full_flow_test.rs     # Happy path tests
    │   └── failure_scenarios.rs  # Error handling tests
    └── README.md                 # This file
```

## Available Tests

### Full Flow Tests (Happy Path)

- `test_full_router_core_flow` - Complete router-core workflow
- `test_router_registry_flow` - Version management
- `test_router_access_control` - Role-based access control
- `test_router_middleware_rate_limiting` - Rate limiting
- `test_router_timelock_operations` - Delayed execution
- `test_timelock_guarded_route_update` - End-to-end timelock-guarded route update security pattern
- `test_router_multicall_batching` - Batch operations
- `test_admin_transfer` - Admin management

### Failure Scenarios

- `test_unauthorized_route_registration` - Access control validation
- `test_duplicate_route_registration` - Duplicate prevention
- `test_resolve_nonexistent_route` - Missing route handling
- `test_invalid_route_name` - Input validation
- `test_paused_router_operations` - Pause state handling
- `test_update_nonexistent_route` - Update validation
- `test_remove_nonexistent_route` - Remove validation
- `test_unauthorized_admin_transfer` - Admin protection
- `test_registry_version_conflict` - Version uniqueness
- `test_access_control_blacklist` - Blacklist enforcement

### Quick Tests

- `test_stellar_cli_available` - Verify CLI installation
- `test_wasm_contracts_built` - Verify WASM files exist
- `test_account_generation_and_funding` - Test account setup

## Test Utilities

### TestAccount

Generate and manage test accounts:

```rust
let account = TestAccount::generate()?;
account.fund()?;  // Fund via Friendbot
```

### DeployedContract

Deploy and interact with contracts:

```rust
let contract = DeployedContract::deploy(
    "path/to/contract.wasm",
    "contract-name",
    &admin_account,
)?;

let result = contract.invoke("method_name", &["--arg", "value"], &admin)?;
```

### TestFixture

Complete test environment:

```rust
let mut fixture = TestFixture::new()?;
fixture.deploy_all_contracts()?;
fixture.initialize_all_contracts()?;

// Access deployed contracts
let core = fixture.router_core.as_ref().unwrap();
```

## Tips

- Use `--nocapture` to see test output
- Use `--test-threads=1` to avoid rate limits
- Tests take 10-30 seconds each due to network latency
- Accounts and contracts remain on testnet after tests

## Documentation

See [INTEGRATION_TESTS.md](../INTEGRATION_TESTS.md) for comprehensive documentation.

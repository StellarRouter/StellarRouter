# integration-tests

This crate contains all integration tests for the `stellar-router` suite. It is
published as `publish = false` and is never deployed — it exists solely to exercise
the contracts from the outside, just as a real caller would.

## Crate layout

```
integration-tests/
├── Cargo.toml                          # Package & dev-dependency declarations
├── README.md                           # This file
├── src/
│   └── lib.rs                          # Shared helpers: TestAccount, DeployedContract,
│                                       #   TestFixture / TestSuite (re-exported for
│                                       #   every test binary below)
└── tests/
    ├── README.md                       # Detailed test-running instructions ← start here
    ├── integration_tests.rs            # Testnet end-to-end happy-path & quick-validation
    │                                   #   tests (require #[ignore]; run via
    │                                   #   cargo test --test integration_tests -- --ignored)
    ├── cross_contract_tests.rs         # Soroban-env cross-contract interaction tests
    │                                   #   (no testnet required)
    ├── failure_scenarios.rs            # Soroban-env contract error / edge-case tests
    │                                   #   (no testnet required)
    ├── quote_execution_tests.rs        # End-to-end router-core → router-quote →
    │                                   #   router-execution pipeline tests
    │                                   #   (no testnet required)
    └── integration/                    # Sub-module used by integration_tests.rs
        ├── mod.rs                      # Module declarations
        ├── testnet_setup.rs            # Testnet account / contract deployment helpers
        ├── full_flow_test.rs           # Happy-path testnet flow tests
        └── failure_scenarios.rs        # Failure-scenario testnet tests
```

### Two flavours of tests

| Location | Runtime | Notes |
|---|---|---|
| `tests/cross_contract_tests.rs`<br>`tests/failure_scenarios.rs`<br>`tests/quote_execution_tests.rs` | Soroban test environment | Fast, no network needed. Run with a plain `cargo test`. |
| `tests/integration_tests.rs` + `tests/integration/` | Stellar testnet | Slow (~10–30 s per test). Tests are marked `#[ignore]` and require a funded testnet account. |

The `tests/integration/failure_scenarios.rs` file is the **testnet** counterpart of
`tests/failure_scenarios.rs`: the former requires a live network and deployed
contracts; the latter runs entirely in the in-process Soroban environment.

## Quick start

```bash
# Soroban-env tests (no testnet, no setup required)
cargo test

# Testnet integration tests
./scripts/run-integration-tests.sh
# or manually:
cargo test --test integration_tests -- --ignored --test-threads=1 --nocapture
```

For full prerequisites, environment variables, and troubleshooting see
[`tests/README.md`](tests/README.md).

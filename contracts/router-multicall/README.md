# router-multicall

`router-multicall` is the Soroban contract in this workspace that groups
cross-contract calls into one invocation and reports which calls succeeded.
Its entry points and types are defined in [`src/lib.rs`](src/lib.rs).

> **Access model:** `execute_batch` is public to **any authenticated caller**;
> it is **not** restricted to the router admin. The admin controls
> configuration and ownership, not which callers are permitted to batch.
> A target contract can still require its own authorization. The multicall
> contract does not grant the caller extra privileges on other contracts.

## Getting started

From the repository root, build or test only this crate:

```bash
cargo build -p router-multicall
cargo test -p router-multicall
```

For a Soroban WASM artifact, install the `wasm32-unknown-unknown` Rust target
and build the workspace's contracts using the commands in the
[contributor guide](../../CONTRIBUTING.md). After deploying this contract,
call `initialize(admin, max_batch_size)` **once**, with a positive batch size.
A second initialization returns `AlreadyInitialized`; zero returns
`InvalidConfig`.

## Executing a batch

```rust
execute_batch(
    caller: Address,
    calls: Vec<CallDescriptor>,
    simulate: bool,
    store_results: bool,
) -> Result<BatchSummary, MulticallError>
```

The caller must authorize the invocation (`caller.require_auth()`). There is
no admin or allowlist check on this entry point. Batches must contain at least
one call and may contain **at most** the configured `max_batch_size`.

Each `CallDescriptor` contains:

| Field | Type | Meaning |
| --- | --- | --- |
| `target` | `Address` | Contract whose function is invoked |
| `function` | `Symbol` | Function to invoke on the target |
| `args` | `Vec<Val>` | Positional arguments passed to the target |
| `required` | `bool` | Whether a failure aborts the entire batch |
| `instruction_budget` | `Option<u64>` | Reserved metadata; **not** an enforced per-call CPU limit |

Calls run in the **input order**. For each attempted call, the router records
whether `env.try_invoke_contract` succeeded and publishes a `call_result` event
with `(caller, target, function, success)` as its data. It does **not** return
the target function's return value in `BatchSummary`.

### Required vs optional calls

- `required: true`: A failed target invocation causes
  `MulticallError::RequiredCallFailed` and aborts the batch. The enclosing
  Soroban transaction fails, so effects of this transaction are rolled back.
  Later calls are not attempted and the completed-batch count is not updated.
- `required: false`: A failed target invocation is counted, but subsequent
  calls are still attempted. The batch can succeed with a mixture of
  successful and failed calls; inspect the summary and events rather than
  assuming every target succeeded.

A failing **optional** call does not by itself abort the outer transaction.
Soroban-level failures (for example, running out of the transaction's overall
resource budget) may still fail the transaction regardless of `required`.

### Execution flags and recorded results

| Flag | Behavior |
| --- | --- |
| `simulate: false` | On successful completion, increments `total_batches` |
| `simulate: true` | Attempts calls normally but **does not** increment `total_batches` |
| `store_results: true` | Stores `CallResult { target, function, success }` under `DataKey::BatchResult(batch_id, call_index)` for each attempted call |
| `store_results: false` | Does not store per-call `CallResult` entries |

**Important:** `simulate: true` is **not** an enforced read-only mode or a
rollback mechanism. The implementation still invokes the target functions
and, when requested, writes result records. Use the Stellar RPC transaction
simulation facility if you need to preview execution without submitting
on-chain effects. The current contract exposes **no public `get_batch_result`
getter**, so clients cannot fetch the stored records through its public
contract API; use events or extend the API if retrieval is required.

## Reading `BatchSummary`

A successful `execute_batch` returns:

| Field | Type | Meaning |
| --- | --- | --- |
| `total` | `u32` | Number of descriptors submitted/attempted |
| `succeeded` | `u32` | Calls whose target invocation succeeded |
| `failed` | `u32` | Calls whose target invocation failed |
| `budget_exceeded_count` | `u32` | **Failed** calls for which `instruction_budget` was set |

For example, one successful call and one failed **optional** call yield
`total = 2`, `succeeded = 1`, and `failed = 1`.

`budget_exceeded_count` is currently a **conservative proxy, not a measured
CPU-overrun count**. Soroban does not expose a guest-contract per-call CPU
meter here: any failed call with `instruction_budget: Some(...)` increments
this counter, even if it failed for a different reason. An actual
transaction-level budget overrun can still revert the whole invocation.

`total_batches()` returns the number of successful *non-simulated* batch
invocations. It starts at zero and is not increased by an aborted required
call or a successful call with `simulate: true`.

## Admin configuration

The admin is specified by `initialize` and must authorize configuration
changes. Public `execute_batch` callers **do not** need to be the admin.

| Entry point | Who can invoke it | Behavior |
| --- | --- | --- |
| `initialize(admin, max_batch_size)` | One-time setup | Requires positive batch size; configures admin and resets count |
| `set_max_batch_size(caller, max_batch_size)` | **Current admin**, authenticated | Changes the maximum; rejects zero; emits `max_batch_size_updated` with `(old, new)` |
| `transfer_admin(current, new_admin)` | **Current admin**, authenticated | Transfers configuration rights; emits `admin_transferred` |
| `admin()` | Public read | Returns current admin; errors when uninitialized |
| `max_batch_size()` | Public read | Returns configured limit; errors when uninitialized |
| `total_batches()` | Public read | Returns completed non-simulated batch count (zero before any completion) |

Only a batch **exceeding** the current maximum returns `BatchTooLarge`;
a batch of exactly `max_batch_size` calls is permitted.

## Errors and troubleshooting

| Error | Typical cause |
| --- | --- |
| `AlreadyInitialized` | `initialize` was called more than once |
| `NotInitialized` | A configured limit or admin is needed before initialization |
| `Unauthorized` | A non-admin tried to change admin-only configuration |
| `BatchTooLarge` | More descriptors than `max_batch_size` |
| `EmptyBatch` | No call descriptors supplied |
| `RequiredCallFailed` | A required target call failed |
| `InvalidConfig` | A zero maximum batch size was supplied |
| `Reentrancy` | An `execute_batch` call was already in progress |

The crate's executable unit tests, including required/optional-call behavior,
admin authorization, summary counts and simulated-batch counters, are in
[`src/lib.rs`](src/lib.rs) under `#[cfg(test)]`. The broader [API reference](../../docs/api-reference.md) and root
[README](../../README.md) provide the
surrounding router architecture.

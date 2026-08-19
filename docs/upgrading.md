# Contract Upgrade Guide

This document describes how to upgrade stellar-router contracts on Soroban,
what to consider for state migration, and the recommended process for each
contract in the suite.

## How Soroban Contract Upgrades Work

Soroban supports in-place WASM replacement via the host function
`update_current_contract_wasm`. When called, the contract's WASM bytecode is
replaced atomically. The contract's storage (all `DataKey` entries) is
**preserved** — the new WASM reads the same storage the old WASM wrote.

This means:
- Adding new storage keys is safe (old entries are simply absent until written).
- Removing storage keys is safe (old entries remain but are ignored).
- **Changing the type of an existing storage key is dangerous** — the new WASM
  will try to deserialize old data with the new type and will panic.
- Changing a `contracterror` discriminant value is a breaking change for
  callers that pattern-match on error codes.

---

## Upgrade Strategy

### Step 1 — Queue the upgrade via router-timelock

Never upgrade a contract directly. Always queue the upgrade as a timelock
operation so there is a delay window for review and cancellation.

```bash
stellar contract invoke --id <TIMELOCK_ID> --network testnet --source admin \
  -- queue \
  --proposer <ADMIN_ADDRESS> \
  --description "upgrade router-core to v2" \
  --target <CORE_CONTRACT_ID> \
  --delay 86400 \
  --depends_on "[]"
```

### Step 2 — Build the new WASM

```bash
cargo build --target wasm32-unknown-unknown --release
```

The new WASM will be at:
```
target/wasm32-unknown-unknown/release/router_core.wasm
```

### Step 3 — Upload the new WASM to the network

```bash
stellar contract upload \
  --wasm target/wasm32-unknown-unknown/release/router_core.wasm \
  --network testnet \
  --source admin
```

This returns a WASM hash. Note it — you will need it in step 4.

### Step 4 — Execute the upgrade after the delay

After the timelock delay has elapsed, execute the operation. The actual
`update_current_contract_wasm` call must be made from within the contract
itself (or via an authorized upgrade function). Add an `upgrade` function
to each contract:

```rust
pub fn upgrade(env: Env, caller: Address, new_wasm_hash: soroban_sdk::BytesN<32>) -> Result<(), RouterError> {
    caller.require_auth();
    Self::require_admin(&env, &caller)?;
    env.deployer().update_current_contract_wasm(new_wasm_hash);
    Ok(())
}
```

Then invoke it:

```bash
stellar contract invoke --id <CORE_ID> --network testnet --source admin \
  -- upgrade \
  --caller <ADMIN_ADDRESS> \
  --new_wasm_hash <WASM_HASH_FROM_STEP_3>
```

---

## State Migration

### Safe changes (no migration needed)

| Change | Safe? | Notes |
|---|---|---|
| Add a new `pub fn` | ✅ | New function, no storage impact |
| Add a new `DataKey` variant | ✅ | Old storage unaffected |
| Add a new `contracterror` variant | ✅ | New discriminant, old callers unaffected |
| Add a field to a struct (with default) | ⚠️ | Only safe if old data can be deserialized — Soroban uses XDR, which is not forward-compatible by default |
| Remove an unused `pub fn` | ✅ | No storage impact |

### Dangerous changes (migration required)

| Change | Risk | Mitigation |
|---|---|---|
| Change the type of an existing `DataKey` value | 🔴 Panic on read | Migrate data before upgrading (see below) |
| Change a `contracterror` discriminant number | 🔴 Breaking for callers | Never reuse discriminant numbers; only add new ones |
| Remove a `DataKey` variant that is still in storage | 🟡 Orphaned data | Acceptable if the data is no longer needed; document it |
| Rename a `contracttype` struct field | 🔴 XDR deserialization failure | Add a new struct, migrate data, remove old struct in a follow-up upgrade |

### Migration pattern

If you need to change a storage type, use a two-phase upgrade:

**Phase 1 — migration upgrade:**
1. Add the new `DataKey` variant (e.g., `RouteEntryV2`).
2. Add a `migrate()` function that reads all `RouteEntry` values, converts them
   to `RouteEntryV2`, writes them under the new key, and removes the old key.
3. Deploy this upgrade.
4. Call `migrate()` once.

**Phase 2 — cleanup upgrade:**
1. Remove the old `DataKey::RouteEntry` variant and all code that references it.
2. Deploy this upgrade.

---

## Per-Contract Upgrade Notes

### router-core

- `RouteEntry` struct has an `Option<RouteMetadata>` field. Adding fields to
  `RouteMetadata` requires a migration if existing entries are stored.
- `DataKey::RouteNames` and `DataKey::Aliases` are `Vec<String>` — safe to
  extend but not to change the element type.
- The `admin()` function panics if the contract is not initialized. Ensure
  `initialize()` has been called before upgrading.

### router-registry

- `ContractEntry` stores `registered_by: Address`. Adding a `registered_at: u64`
  timestamp field requires a migration for existing entries.
- Version lists (`DataKey::Versions`) are `Vec<u32>` — safe to extend.

### router-access

- `DataKey::RoleParent` is new in the hierarchy feature. Old deployments without
  it will simply have no parent relationships — safe to add without migration.
- `DataKey::HasRole` stores `bool`. Do not change this to a struct without a
  migration.

### router-middleware

- `RouteConfig` has grown over time (added `failure_threshold`,
  `recovery_window_seconds`, `log_retention`). If upgrading from an older
  deployment, existing `RouteConfig` entries will fail to deserialize with the
  new struct. Run a migration that re-writes all `RouteConfig` entries with
  default values for the new fields.

### router-timelock

- `TimelockOp` has `is_critical: bool`. Old entries without this field will
  fail to deserialize. If upgrading from a pre-hierarchy deployment, migrate
  all existing operations to set `is_critical = false`.
- `DataKey::FastTrackEnabled` is new — safe to add without migration.

### router-multicall

- `CallDescriptor` has `instruction_budget: Option<u64>`. Old entries without
  this field will fail to deserialize if stored. Since `execute_batch` does not
  persist `CallDescriptor` values, this is safe.

---

## Rollback

Soroban does not support automatic rollback of a WASM upgrade. If an upgrade
introduces a bug:

1. Build the previous WASM version.
2. Upload it to the network (step 3 above).
3. Call `upgrade()` with the old WASM hash.

This is why all upgrades should be queued through router-timelock — the delay
window gives time to test the new WASM on testnet and cancel the upgrade if
issues are found before it executes on mainnet.

---

## Upgrade Checklist

Before upgrading any contract on mainnet:

- [ ] New WASM tested on testnet with production-like data
- [ ] Storage compatibility verified (no type changes without migration)
- [ ] `contracterror` discriminants unchanged
- [ ] Upgrade queued via router-timelock with at least 24h delay
- [ ] Migration function (if needed) tested on testnet
- [ ] Rollback WASM uploaded and hash noted
- [ ] On-chain monitoring active during upgrade window

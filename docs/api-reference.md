# API Reference

Complete reference for all public functions across the six stellar-router contracts.

---

## Error Code Reference

Soroban contract errors are emitted as numeric enum discriminants. Error names
and values are scoped to each contract, so the same numeric value can mean
different things depending on which contract returned it.

### router-core `RouterError`

| Error | Value | When it occurs | How to handle it |
| --- | ---: | --- | --- |
| `AlreadyInitialized` | 1 | `initialize` is called after the router has already been initialized. | Treat initialization as complete, or deploy a fresh contract if you need a new admin. |
| `NotInitialized` | 2 | Admin-only helpers are called before `initialize` has stored the admin. | Initialize the router before calling configuration or admin reads. |
| `Unauthorized` | 3 | The caller is not the stored router admin. | Sign with the admin account or transfer admin first. |
| `RouteNotFound` | 4 | A route or alias lookup targets a name that is not registered. | Register the route, check spelling, or remove stale aliases in the caller. |
| `RoutePaused` | 5 | `resolve` targets a route that has been individually paused. | Unpause the route or route traffic elsewhere. |
| `RouterPaused` | 6 | `resolve` is called while the global router pause is active. | Wait for recovery or have the admin call `set_paused(false)`. |
| `RouteAlreadyExists` | 7 | Registering a route or alias with a name that is already in use. | Choose a unique name or update/remove the existing route first. |
| `InvalidRouteName` | 8 | A route name is empty or only whitespace. | Send a non-empty, trimmed route name. |
| `InvalidMetadata` | 9 | Route metadata exceeds limits: description over 256 characters or more than 5 tags. | Shorten the description or reduce the tag list before submitting. |

### router-registry `RegistryError`

| Error | Value | When it occurs | How to handle it |
| --- | ---: | --- | --- |
| `AlreadyInitialized` | 1 | `initialize` is called after the registry has already been initialized. | Skip initialization or deploy a fresh registry for a new admin. |
| `NotInitialized` | 2 | Admin-dependent registry operations run before initialization. | Initialize the registry before writes or admin reads. |
| `Unauthorized` | 3 | The caller is not the registry admin. | Sign with the admin account or transfer admin first. |
| `NotFound` | 4 | No entry exists for the requested name/version, or no version satisfies a lookup. | Register the contract or adjust the requested name, version, or constraint. |
| `AlreadyRegistered` | 5 | The same `(name, version)` pair is registered twice. | Use a new version or deprecate old entries instead of overwriting. |
| `AlreadyDeprecated` | 6 | `deprecate` targets an entry that is already deprecated. | Treat the operation as already complete or choose another version. |
| `InvalidVersion` | 7 | Version is `0` or not greater than the current highest version for the name. | Use a positive version greater than existing versions. |
| `VersionNotFound` | 8 | `deprecate` targets a version that is not registered for the name. | Check `versions(name)` and retry with an existing version. |
| `InvalidConstraint` | 9 | A semver constraint cannot be parsed or uses an unsupported form. | Use exact, `>`, `>=`, `<`, `<=`, `^`, or `~` constraints. |
| `AllVersionsDeprecated` | 10 | Lookup finds versions, but every matching entry is deprecated. | Register a newer active version or allow deprecated versions in the caller logic. |

### router-access `AccessError`

| Error | Value | When it occurs | How to handle it |
| --- | ---: | --- | --- |
| `AlreadyInitialized` | 1 | `initialize` is called after the access contract has already been initialized. | Skip initialization or deploy a fresh access contract. |
| `NotInitialized` | 2 | Admin or role checks require the super-admin before it is stored. | Initialize the contract with a super-admin first. |
| `Unauthorized` | 3 | The caller lacks super-admin or role-admin authority for the action. | Use an authorized signer or grant the required admin role first. |
| `AlreadyHasRole` | 4 | `grant_role` targets an address that already holds the role directly. | Treat as success, wait for expiry, or revoke before granting again. |
| `RoleNotFound` | 5 | `revoke_role` targets an address without that direct role grant. | Check `has_role` or role member lists before revoking. |
| `Blacklisted` | 6 | A blacklisted address is granted or checked for role access. | Unblacklist the address before granting roles. |
| `CannotBlacklistAdmin` | 7 | The caller tries to blacklist the super-admin address. | Transfer super-admin first, or choose another target. |
| `HierarchyCycle` | 8 | Setting a role admin would create a parent-role cycle. | Pick a role-admin hierarchy that does not point back to itself. |

### router-middleware `MiddlewareError`

| Error | Value | When it occurs | How to handle it |
| --- | ---: | --- | --- |
| `AlreadyInitialized` | 1 | `initialize` is called after middleware has already been initialized. | Skip initialization or deploy a fresh middleware contract. |
| `NotInitialized` | 2 | Middleware configuration or admin reads run before initialization. | Initialize the contract before route configuration. |
| `Unauthorized` | 3 | The caller is not the middleware admin. | Sign with the admin account or transfer admin first. |
| `RateLimitExceeded` | 4 | A caller has used all allowed calls for the route's current window. | Retry after the window resets or raise the configured limit. |
| `RouteDisabled` | 5 | `pre_call` runs against a route whose middleware config is disabled. | Enable the route or bypass it intentionally. |
| `MiddlewareDisabled` | 6 | Global middleware enforcement is disabled. | Re-enable middleware or treat the route as temporarily unavailable. |
| `InvalidConfig` | 7 | Rate limiting is enabled with `max_calls_per_window > 0` but `window_seconds == 0`. | Set a positive window or set `max_calls_per_window` to `0` for unlimited calls. |
| `CircuitOpen` | 8 | The route's circuit breaker is open after repeated failures. | Wait for the recovery window, reset the breaker, or fix the failing downstream route. |

### router-timelock `TimelockError`

| Error | Value | When it occurs | How to handle it |
| --- | ---: | --- | --- |
| `AlreadyInitialized` | 1 | `initialize` is called after the timelock has already been initialized. | Skip initialization or deploy a fresh timelock contract. |
| `NotInitialized` | 2 | Timelock operations need the admin or delay before initialization. | Initialize the timelock with an admin and minimum delay. |
| `Unauthorized` | 3 | The caller is not the timelock admin. | Sign with the admin account or transfer admin first. |
| `NotFound` | 4 | `execute` or `cancel` targets an unknown operation id. | Recompute the operation id or query the operation before acting. |
| `NotReady` | 5 | `execute` is called before the queued operation's ETA. | Wait until the ETA has passed. |
| `AlreadyExecuted` | 6 | `execute` or `cancel` targets an operation that has already executed. | Treat it as complete and avoid replaying the operation. |
| `Cancelled` | 7 | `execute` or `cancel` targets an operation that has been cancelled. | Queue a new operation if the action is still needed. |
| `DelayTooShort` | 8 | `queue` uses a delay lower than the configured minimum delay. | Use at least `min_delay()` seconds. |

### router-multicall `MulticallError`

| Error | Value | When it occurs | How to handle it |
| --- | ---: | --- | --- |
| `AlreadyInitialized` | 1 | `initialize` is called after multicall has already been initialized. | Skip initialization or deploy a fresh multicall contract. |
| `NotInitialized` | 2 | Batch execution or admin reads require config before initialization. | Initialize the contract with an admin and batch size. |
| `Unauthorized` | 3 | The caller is not the multicall admin for configuration changes. | Sign with the admin account or transfer admin first. |
| `BatchTooLarge` | 4 | `execute_batch` receives more calls than `max_batch_size`. | Split the batch or increase `max_batch_size`. |
| `EmptyBatch` | 5 | `execute_batch` is called with no calls. | Provide at least one call descriptor. |
| `RequiredCallFailed` | 6 | A required call in the batch fails. | Inspect per-call results, fix the failed required call, or mark it optional if safe. |
| `InvalidConfig` | 7 | `max_batch_size` is set to `0`. | Configure a positive maximum batch size. |

---

## router-core

**Contract:** `RouterCore`  
**Purpose:** Central dispatcher — registers routes by name and resolves them to contract addresses.

### `initialize(admin: Address) → Result<(), RouterError>`
Sets up the admin, marks the router as unpaused, and resets the total-routed counter.  
Must be called exactly once before any other function.

**Errors:** `AlreadyInitialized`

```bash
stellar contract invoke --id <CORE_ID> --network testnet --source admin \
  -- initialize --admin <ADMIN_ADDRESS>
```

---

### `register_route(caller, name, address) → Result<(), RouterError>`
Registers a new route. `name` must be unique and non-empty. Caller must be admin.

**Errors:** `Unauthorized`, `RouteAlreadyExists`, `NotInitialized`, `InvalidRouteName` (empty/whitespace)

```bash
stellar contract invoke --id <CORE_ID> --network testnet --source admin \
  -- register_route --caller <ADMIN> --name oracle --address <CONTRACT_ID>
```

---

### `update_route(caller, name, new_address) → Result<(), RouterError>`
Updates an existing route to point to a new address. Emits `route_updated` and `route_overwritten` events.

**Errors:** `Unauthorized`, `RouteNotFound`, `NotInitialized`

---

### `remove_route(caller, name) → Result<(), RouterError>`
Deletes a route and removes any aliases pointing to it.

**Errors:** `Unauthorized`, `RouteNotFound`, `NotInitialized`

---

### `resolve(name) → Result<Address, RouterError>`
Resolves a route name (or alias) to its contract address. Increments `total_routed`.

**Errors:** `RouterPaused`, `RouteNotFound`, `RoutePaused`

```bash
stellar contract invoke --id <CORE_ID> --network testnet --source any \
  -- resolve --name oracle
```

---

### `set_route_paused(caller, name, paused: bool) → Result<(), RouterError>`
Pauses or unpauses a specific route.

**Errors:** `Unauthorized`, `RouteNotFound`, `NotInitialized`

---

### `set_paused(caller, paused: bool) → Result<(), RouterError>`
Pauses or unpauses the entire router. Overrides individual route state.

**Errors:** `Unauthorized`, `NotInitialized`

---

### `get_route(name) → Option<RouteEntry>`
Returns the full `RouteEntry` for `name`, or `None` if not registered.

---

### `get_all_routes() → Vec<String>`
Returns all registered route names.

---

### `add_alias(caller, existing_name, alias_name) → Result<(), RouterError>`
Creates an alias for an existing route. Resolving the alias returns the original route's address.

**Errors:** `Unauthorized`, `RouteNotFound` (existing_name), `RouteAlreadyExists` (alias_name)

---

### `remove_alias(caller, alias_name) → Result<(), RouterError>`
Removes an alias.

**Errors:** `Unauthorized`, `RouteNotFound`

---

### `total_routed() → u64`
Returns the cumulative count of successful `resolve` calls.

---

### `admin() → Result<Address, RouterError>`
Returns the current admin address.

**Errors:** `NotInitialized`

---

### `transfer_admin(current, new_admin) → Result<(), RouterError>`
Transfers admin to a new address. Emits `admin_transferred`.

**Errors:** `Unauthorized`, `NotInitialized`

---

## router-registry

**Contract:** `RouterRegistry`  
**Purpose:** Versioned address book — stores contract addresses keyed by `(name, version)`.

### `initialize(admin) → Result<(), RegistryError>`
**Errors:** `AlreadyInitialized`

---

### `register(caller, name, address, version: u32) → Result<(), RegistryError>`
Registers a contract entry. `version` must be > 0 and greater than all existing versions for `name`.

**Errors:** `Unauthorized`, `InvalidVersion`, `AlreadyRegistered`, `NotInitialized`

```bash
stellar contract invoke --id <REGISTRY_ID> --network testnet --source admin \
  -- register --caller <ADMIN> --name oracle --address <CONTRACT_ID> --version 1
```

---

### `get(name, version: u32) → Result<ContractEntry, RegistryError>`
Returns the entry for `(name, version)`.

**Errors:** `NotFound`

---

### `get_latest(name) → Result<ContractEntry, RegistryError>`
Returns the highest non-deprecated version for `name`.

**Errors:** `NotFound`

---

### `get_latest_with_constraint(name, constraint: Option<String>) → Result<ContractEntry, RegistryError>`
Returns the highest non-deprecated version matching a semver constraint (`>=X`, `<=X`, `>X`, `<X`, `^X`, `~X`, or exact).

**Errors:** `NotFound`, `InvalidConstraint`

---

### `deprecate(caller, name, version: u32) → Result<(), RegistryError>`
Marks a version as deprecated. Deprecated versions are skipped by `get_latest`.

**Errors:** `Unauthorized`, `VersionNotFound`, `AlreadyDeprecated`, `NotInitialized`

---

### `deprecate_many(caller, entries: Vec<(String, u32)>) → Vec<Result<(), RegistryError>>`
Batch deprecation. Returns one result per entry; partial failures are allowed.

---

### `versions(name) → Vec<u32>`
Returns all version numbers registered under `name` in ascending order. Returns an empty vec if `name` has no registered entries.

---

### `deprecate_all_versions(caller, name, reason: Option<String>) → Result<(), RegistryError>`
Deprecates every version registered under `name` in one call. Already-deprecated versions are silently skipped.

**Errors:** `Unauthorized`, `NotInitialized`

---

### `admin() → Result<Address, RegistryError>`
**Errors:** `NotInitialized`

---

### `transfer_admin(current, new_admin) → Result<(), RegistryError>`
**Errors:** `Unauthorized`, `NotInitialized`

---

## router-access

**Contract:** `RouterAccess`  
**Purpose:** Role-based access control with optional role expiry and blacklisting.

### `initialize(super_admin) → Result<(), AccessError>`
**Errors:** `AlreadyInitialized`

---

### `grant_role(caller, role, target, expires_at: Option<u64>) → Result<(), AccessError>`
Grants `role` to `target`. `expires_at` is an optional ledger sequence number after which the role expires. Caller must be super-admin or role admin.

**Errors:** `Unauthorized`, `AlreadyHasRole`, `Blacklisted`

---

### `revoke_role(caller, role, target) → Result<(), AccessError>`
Revokes `role` from `target`. Removes the storage key (not just sets to false).

**Errors:** `Unauthorized`, `RoleNotFound`

---

### `has_role(role, target) → bool`
Returns `true` if `target` holds `role` and it has not expired. Returns `false` for blacklisted addresses.

---

### `set_role_admin(caller, role, admin) → Result<(), AccessError>`
Designates `admin` as the address that can grant/revoke `role`. Only super-admin can call this.

**Errors:** `Unauthorized`, `NotInitialized`

---

### `blacklist(caller, target) → Result<(), AccessError>`
Prevents `target` from being granted any role. Cannot blacklist the super-admin.

**Errors:** `Unauthorized`, `CannotBlacklistAdmin`, `NotInitialized`

---

### `unblacklist(caller, target) → Result<(), AccessError>`
Removes `target` from the blacklist.

**Errors:** `Unauthorized`, `NotInitialized`

---

### `is_blacklisted(target) → bool`

---

### `get_role_members(role) → Vec<Address>`
Returns all addresses currently holding `role`.

---

### `get_roles_for_address(addr) → Vec<String>`
Returns all roles held by `addr`.

---

### `expire_role(caller, role, target) → Result<(), AccessError>`
Removes the expiry entry for a role grant (cleanup function). Only super-admin.

**Errors:** `Unauthorized`, `NotInitialized`

---

### `super_admin() → Result<Address, AccessError>`
**Errors:** `NotInitialized`

---

### `transfer_super_admin(current, new_admin) → Result<(), AccessError>`
**Errors:** `Unauthorized`, `NotInitialized`

---

## router-quote

**Contract:** `RouterQuote`
**Purpose:** Read-only quote previews for single-hop and multi-hop routes, including expected output, fees, slippage bounds, and a fixed-point exchange rate.

### `get_quote(router_core, route_name, token_in, token_out, amount_in, fee_bps, slippage_bps, precision) → Result<QuoteResponse, QuoteError>`
Returns a quote for one liquidity plugin route. The call previews the swap result only; it does not execute a transfer.

**Errors:** `InvalidAmount`, `InvalidPrecision`, `InvalidSlippage`, `InvalidRoute`, `RouteNotFound`, `QuoteFailed`

---

### `get_multihop_quote(hops, amount_in, slippage_bps, precision) → Result<QuoteResponse, QuoteError>`
Returns an end-to-end quote for an ordered route of liquidity plugin hops. The `amount_out` from each hop becomes the `amount_in` for the next hop.

**Errors:** `EmptyRoute`, `RouteTooLong`, `InvalidAmount`, `InvalidPrecision`, `InvalidSlippage`, `QuoteFailed`

---

### Quote Precision And Rounding

`exchange_rate` is returned as a fixed-point integer instead of a floating-point decimal:

```text
exchange_rate = floor(amount_out * 10^precision / amount_in)
```

`precision` is the number of decimal places encoded in `exchange_rate`. The contract accepts precision values from `1` through `18`; `6` is the typical default for UI and API clients.

For tokens with the same decimal count, convert the returned value to a human-readable rate like this:

```text
display_rate = exchange_rate / 10^precision
```

For tokens with different decimal counts, adjust for token base units:

```text
display_rate = (exchange_rate / 10^precision) * 10^(token_in_decimals - token_out_decimals)
```

Example with equal token decimals:

| `amount_in` | `amount_out` | `precision` | `exchange_rate` | Display rate |
|---:|---:|---:|---:|---:|
| `1_000000` | `2_000000` | `6` | `2_000000` | `2.000000` |
| `3_000000` | `1_000000` | `6` | `333333` | `0.333333` |

Rounding is deterministic and always truncates toward zero because Soroban integer division is used. The contract does not round up fractional remainders, so clients should treat the value as a floor estimate of the exact rate. Fee amounts and `min_amount_out` use the same integer-division behavior:

```text
fee_amount = floor(amount_in * fee_bps / 10_000)
min_amount_out = floor(amount_out * (10_000 - slippage_bps) / 10_000)
```

For UI display, keep the integer values for transaction decisions and apply formatting only at the presentation layer. For comparisons, compare fixed-point integers at the same `precision` rather than converting through floating-point numbers.

---

## router-middleware

**Contract:** `RouterMiddleware`  
**Purpose:** Pre/post call hooks with rate limiting and circuit breaker.

### `initialize(admin) → Result<(), MiddlewareError>`
**Errors:** `AlreadyInitialized`

---

### `configure_route(caller, route, max_calls_per_window, window_seconds, enabled, failure_threshold, recovery_window_seconds) → Result<(), MiddlewareError>`
Configures rate limiting and circuit breaker for a route. Set `max_calls_per_window = 0` to disable rate limiting. Set `failure_threshold = 0` to disable the circuit breaker.

**Errors:** `Unauthorized`, `InvalidConfig` (window_seconds=0 with max_calls>0), `NotInitialized`

```bash
stellar contract invoke --id <MIDDLEWARE_ID> --network testnet --source admin \
  -- configure_route \
  --caller <ADMIN> --route oracle/get_price \
  --max_calls_per_window 100 --window_seconds 3600 \
  --enabled true --failure_threshold 5 --recovery_window_seconds 300
```

---

### `pre_call(caller, route) → Result<(), MiddlewareError>`
Must be called before routing. Validates global enable, route enable, circuit breaker, and rate limit. Increments `total_calls` on success.

**Errors:** `MiddlewareDisabled`, `RouteDisabled`, `CircuitOpen`, `RateLimitExceeded`

---

### `post_call(caller, route, success: bool)`
Must be called after routing. Emits `post_call` event. Increments circuit breaker failure count on failure.

---

### `set_global_enabled(caller, enabled: bool) → Result<(), MiddlewareError>`
Globally enables or disables all middleware.

**Errors:** `Unauthorized`, `NotInitialized`

---

### `reset_circuit_breaker(caller, route) → Result<(), MiddlewareError>`
Manually resets the circuit breaker for a route.

**Errors:** `Unauthorized`, `NotInitialized`

---

### `total_calls() → u64`
Cumulative count of successful `pre_call` invocations.

---

### `rate_limit_state(route, caller) → Option<RateLimitState>`
Returns the current rate limit state for `(route, caller)`.

---

### `route_config(route) → Option<RouteConfig>`
Returns the middleware config for `route`.

---

### `admin() → Result<Address, MiddlewareError>`
**Errors:** `NotInitialized`

---

### `transfer_admin(current, new_admin) → Result<(), MiddlewareError>`
**Errors:** `Unauthorized`, `NotInitialized`

---

## router-timelock

**Contract:** `RouterTimelock`  
**Purpose:** Delayed execution queue — all sensitive changes must wait a configurable delay.

### `initialize(admin, min_delay: u64) → Result<(), TimelockError>`
`min_delay` must be > 0 (seconds).

**Errors:** `AlreadyInitialized`, `InvalidDelay`

---

### `queue(proposer, description, target, delay: u64, depends_on: Vec<u64>) → Result<u64, TimelockError>`
Queues a new operation. Returns the operation ID. `delay` must be >= `min_delay`. `depends_on` lists operation IDs that must execute first.

**Errors:** `Unauthorized`, `InvalidDelay`, `NotInitialized`

```bash
stellar contract invoke --id <TIMELOCK_ID> --network testnet --source admin \
  -- queue \
  --proposer <ADMIN> --description "upgrade oracle to v2" \
  --target <CONTRACT_ID> --delay 86400 --depends_on "[]"
```

---

### `execute(caller, op_id: u64) → Result<(), TimelockError>`
Executes a queued operation after its ETA. All dependencies must be executed first.

**Errors:** `Unauthorized`, `NotFound`, `AlreadyExecuted`, `AlreadyCancelled`, `TooEarly`, `DependencyNotMet`

---

### `cancel(caller, op_id: u64) → Result<(), TimelockError>`
Cancels a queued operation. Clears its dependency list.

**Errors:** `Unauthorized`, `NotFound`, `AlreadyExecuted`, `AlreadyCancelled`

---

### `cancel_all(admin) → Result<u32, TimelockError>`
Cancels all pending (not executed, not cancelled) operations. Returns the count cancelled.

**Errors:** `Unauthorized`, `NotInitialized`

---

### `get_op(op_id: u64) → Option<TimelockOp>`
Returns the operation, or `None` if not found.

---

### `min_delay() → Result<u64, TimelockError>`
**Errors:** `NotInitialized`

---

### `set_min_delay(caller, new_delay: u64) → Result<(), TimelockError>`
Updates the minimum delay. Does not affect already-queued operations.

**Errors:** `Unauthorized`, `InvalidDelay`, `NotInitialized`

---

### `admin() → Result<Address, TimelockError>`
**Errors:** `NotInitialized`

---

### `transfer_admin(current, new_admin) → Result<(), TimelockError>`
**Errors:** `Unauthorized`, `NotInitialized`

---

## router-multicall

**Contract:** `RouterMulticall`  
**Purpose:** Batch multiple cross-contract calls in a single transaction.

### `initialize(admin, max_batch_size: u32) → Result<(), MulticallError>`
`max_batch_size` must be > 0.

**Errors:** `AlreadyInitialized`, `InvalidConfig`

---

### `execute_batch(caller, calls: Vec<CallDescriptor>, simulate: bool) → Result<BatchSummary, MulticallError>`
Executes a batch of calls. Any authenticated address can call this (not admin-only). If `simulate = true`, calls are attempted but `total_batches` is not incremented. If a call with `required = true` fails, the batch aborts immediately.

**Errors:** `EmptyBatch`, `BatchTooLarge`, `RequiredCallFailed`, `NotInitialized`

---

### `set_max_batch_size(caller, max_batch_size: u32) → Result<(), MulticallError>`
**Errors:** `Unauthorized`, `InvalidConfig`, `NotInitialized`

---

### `total_batches() → u64`
Cumulative count of non-simulated `execute_batch` calls.

---

### `max_batch_size() → Result<u32, MulticallError>`
**Errors:** `NotInitialized`

---

### `admin() → Result<Address, MulticallError>`
**Errors:** `NotInitialized`

---

### `transfer_admin(current, new_admin) → Result<(), MulticallError>`
**Errors:** `Unauthorized`, `NotInitialized`

---

## router-quote

**Contract:** `RouterQuote`  
**Purpose:** Price quoting for token swaps — calculates expected output amounts, fees, and exchange rates without executing transactions.

### `get_quote(plugin, route_name, token_in, token_out, amount_in, fee_bps, slippage_bps, precision) → Result<QuoteResponse, QuoteError>`

Gets a single-hop quote from a liquidity plugin. Returns expected output, fees, exchange rate, and price impact.

**Parameters:**
- `plugin` - Address of the liquidity plugin contract
- `route_name` - Route identifier
- `token_in` - Input token address
- `token_out` - Output token address
- `amount_in` - Amount to swap (must be > 0)
- `fee_bps` - Protocol fee in basis points
- `slippage_bps` - Slippage tolerance in basis points (0–10000)
- `precision` - Decimal places for exchange rate (1–18, typically 6)

**Errors:** `InvalidAmount`, `InvalidPrecision`, `InvalidSlippage`, `QuoteFailed`, `RouteNotFound`

---

### `get_multihop_quote(hops, amount_in, slippage_bps, precision) → Result<QuoteResponse, QuoteError>`

Gets a multi-hop quote chaining multiple liquidity plugins. Returns end-to-end exchange rate and per-hop breakdown.

**Parameters:**
- `hops` - Ordered list of `HopDescriptor` (1–5 hops)
- `amount_in` - Initial input amount (must be > 0)
- `slippage_bps` - Slippage tolerance applied to final output (0–10000)
- `precision` - Decimal places for end-to-end exchange rate (1–18)

**Errors:** `EmptyRoute`, `RouteTooLong`, `InvalidAmount`, `InvalidPrecision`, `InvalidSlippage`, `QuoteFailed`

---

### Exchange Rate Precision and Rounding

The `exchange_rate` field uses **fixed-point arithmetic** to represent decimal values as integers, avoiding floating-point precision issues on-chain.

#### Formula

```
exchange_rate = (amount_out * 10^precision) / amount_in
```

#### Precision Values

- **Range:** 1–18 decimal places
- **Typical value:** 6 (supports micro-precision)
- **Configured per-quote:** Caller specifies precision when requesting a quote

#### Converting to Decimal

To convert the fixed-point `exchange_rate` to a decimal value:

```
decimal_rate = exchange_rate / 10^precision
```

**Examples:**
- `exchange_rate = 2_000_000`, `precision = 6` → `2.000000`
- `exchange_rate = 1_050_000`, `precision = 6` → `1.050000`
- `exchange_rate = 200`, `precision = 2` → `2.00`

#### Rounding Behavior

The calculation uses **integer division**, which **truncates (rounds down)** toward zero. This is deterministic and avoids banker's rounding or floating-point ambiguity.

**Example with truncation:**
```
amount_in = 3
amount_out = 10
precision = 6

exchange_rate = (10 * 1_000_000) / 3 = 3_333_333
decimal_rate = 3.333333 (truncated from 3.333333...)
```

#### Token Decimal Considerations

When working with tokens that have different decimal places:

1. **Token amounts** are in the token's native units (e.g., 1 USDC = 1_000_000 units for 6 decimals)
2. **Exchange rate precision** is independent of token decimals — it controls the rate's decimal representation
3. **Conversion example:**
   - Token A has 6 decimals, Token B has 18 decimals
   - Swap 1_000_000 Token A (1.0 Token A) for 5_000_000_000_000_000_000 Token B (5.0 Token B)
   - With `precision = 6`: `exchange_rate = (5_000_000_000_000_000_000 * 10^6) / 1_000_000 = 5_000_000_000_000_000`
   - Decimal rate: `5.0` (5 Token B per 1 Token A)

#### Best Practices

- Use `precision = 6` for most use cases (micro-precision)
- Use higher precision (12–18) for high-value or low-decimal tokens
- Always check the `precision` field when interpreting `exchange_rate`
- Account for truncation when calculating expected outputs client-side

---

## Error Code Reference

Each contract defines its own `#[contracterror]` enum. Use the tables below as the canonical error-code reference for integration, monitoring, and client-side handling.

### router-core (`RouterError`)

| Error | Code | When it occurs | How to handle |
|---|---:|---|---|
| `AlreadyInitialized` | `1` | `initialize` is called after admin/state already exists | Treat as idempotent setup; skip re-initialization and proceed with existing deployment |
| `NotInitialized` | `2` | Admin-gated write methods run before first initialization | Initialize the contract first, then retry |
| `Unauthorized` | `3` | Caller is not current admin for admin-only methods | Re-submit with admin signer or transfer admin before retry |
| `RouteNotFound` | `4` | Route (or alias target) does not exist for lookup/update/remove | Register route first, or correct the route/alias name |
| `RoutePaused` | `5` | `resolve` is called for a route marked paused | Unpause that route, or route traffic to another available route |
| `RouterPaused` | `6` | Router is globally paused during `resolve`/route selection | Unpause globally before serving traffic |
| `RouteAlreadyExists` | `7` | Registering route or alias that conflicts with existing route/alias | Use a unique name, or update/remove existing entry first |
| `InvalidRouteName` | `8` | Route name is empty or whitespace-only | Validate/sanitize names client-side before submit |
| `InvalidMetadata` | `9` | Route metadata exceeds constraints (description/tags limits) | Trim metadata to allowed bounds, then retry |

### router-registry (`RegistryError`)

| Error | Code | When it occurs | How to handle |
|---|---:|---|---|
| `AlreadyInitialized` | `1` | `initialize` called more than once | Treat as already configured; do not re-run init |
| `NotInitialized` | `2` | Admin-gated write method called before initialization | Initialize first, then retry |
| `Unauthorized` | `3` | Non-admin caller attempts register/deprecate/admin transfer | Use admin signer or perform admin handoff |
| `NotFound` | `4` | Requested `(name, version)` or constrained lookup has no match | Check name/version/constraint inputs and fallback strategy |
| `AlreadyRegistered` | `5` | `(name, version)` already exists | Bump version or update workflow instead of duplicate register |
| `AlreadyDeprecated` | `6` | Deprecating an entry that is already deprecated | Treat as idempotent deprecation and continue |
| `InvalidVersion` | `7` | Version is `0` or not strictly greater than existing versions | Submit monotonically increasing, non-zero version |
| `VersionNotFound` | `8` | Deprecating a version that does not exist | Verify version list with `versions(name)` first |
| `InvalidConstraint` | `9` | Constraint string format is invalid | Validate/normalize constraint syntax before calling |
| `AllVersionsDeprecated` | `10` | Lookup finds only deprecated versions for a name | Register a new active version or allow deprecated fallback intentionally |

### router-access (`AccessError`)

| Error | Code | When it occurs | How to handle |
|---|---:|---|---|
| `AlreadyInitialized` | `1` | `initialize` called after super-admin already set | Treat as already initialized |
| `NotInitialized` | `2` | Operations requiring super-admin state run before init | Initialize first, then retry |
| `Unauthorized` | `3` | Caller is not super-admin or configured role admin | Use an authorized account or update role-admin assignment |
| `AlreadyHasRole` | `4` | Granting role already held (directly/inherited as implemented) | Treat as idempotent grant and continue |
| `RoleNotFound` | `5` | Revoking/removing role that is not present | Confirm membership before revoke, or treat as already removed |
| `Blacklisted` | `6` | Target/caller is blacklisted for role-management operation | Unblacklist or use a non-blacklisted account |
| `CannotBlacklistAdmin` | `7` | Attempt to blacklist current super-admin | Transfer super-admin first (if needed), then blacklist old admin |
| `HierarchyCycle` | `8` | Setting role parent would create cycle | Redesign role graph to remain acyclic |

### router-middleware (`MiddlewareError`)

| Error | Code | When it occurs | How to handle |
|---|---:|---|---|
| `AlreadyInitialized` | `1` | `initialize` called after admin already configured | Treat as already initialized |
| `NotInitialized` | `2` | Admin-only configuration called before initialization | Initialize first, then retry |
| `Unauthorized` | `3` | Non-admin caller tries config/global/circuit/admin updates | Use admin signer |
| `RateLimitExceeded` | `4` | `pre_call` exceeds configured per-caller route window | Back off until next window; implement retry with jitter |
| `RouteDisabled` | `5` | `pre_call` on a route configured as disabled | Re-enable route or route to alternate endpoint |
| `MiddlewareDisabled` | `6` | Global middleware toggle is disabled | Re-enable global middleware before routed calls |
| `InvalidConfig` | `7` | Invalid route config (e.g. non-zero max calls with zero window) | Validate config invariants client-side before write |
| `CircuitOpen` | `8` | Circuit breaker open for route and recovery window not elapsed | Wait for recovery window or reset circuit via admin |

### router-timelock (`TimelockError`)

| Error | Code | When it occurs | How to handle |
|---|---:|---|---|
| `AlreadyInitialized` | `1` | `initialize` called after admin already set | Treat as already initialized |
| `NotInitialized` | `2` | Admin checks/storage reads run before initialization | Initialize first, then retry |
| `Unauthorized` | `3` | Non-admin tries queue/cancel/execute | Use timelock admin signer |
| `NotFound` | `4` | Operation ID not present in storage | Verify op ID from queue event/output before acting |
| `NotReady` | `5` | `execute` called before operation ETA | Wait until ETA then retry |
| `AlreadyExecuted` | `6` | Execute/cancel attempted on operation already executed | Treat as terminal completed state |
| `Cancelled` | `7` | Execute/cancel attempted on already-cancelled operation | Treat as terminal cancelled state |
| `DelayTooShort` | `8` | Queue delay is below configured `min_delay` | Submit with `delay >= min_delay` |

### router-multicall (`MulticallError`)

| Error | Code | When it occurs | How to handle |
|---|---:|---|---|
| `AlreadyInitialized` | `1` | `initialize` called after admin exists | Treat as already initialized |
| `NotInitialized` | `2` | Batch/config call before initial setup | Initialize first, then retry |
| `Unauthorized` | `3` | Non-admin calls admin-only config methods | Use admin signer for config operations |
| `BatchTooLarge` | `4` | `execute_batch` call count exceeds configured max | Split calls into smaller batches or raise max as admin |
| `EmptyBatch` | `5` | `execute_batch` called with zero calls | Validate non-empty input before submit |
| `RequiredCallFailed` | `6` | A call marked `required=true` fails and aborts batch | Retry after fixing failing target/function/input, or mark optional if acceptable |
| `InvalidConfig` | `7` | Invalid config such as `max_batch_size = 0` | Enforce positive config values client-side |

---

## Common Types

### `RouteEntry`
| Field | Type | Description |
|---|---|---|
| `address` | `Address` | Resolved contract address |
| `name` | `String` | Route name |
| `paused` | `bool` | Whether this route is paused |
| `updated_by` | `Address` | Last admin to update this route |

### `ContractEntry`
| Field | Type | Description |
|---|---|---|
| `address` | `Address` | Registered contract address |
| `name` | `String` | Human-readable name |
| `version` | `u32` | Version number |
| `deprecated` | `bool` | Whether deprecated |
| `registered_by` | `Address` | Who registered it |

### `CallDescriptor`
| Field | Type | Description |
|---|---|---|
| `target` | `Address` | Contract to call |
| `function` | `Symbol` | Function name |
| `required` | `bool` | Abort batch on failure |
| `instruction_budget` | `Option<u64>` | Reserved for future budget metering |

### `BatchSummary`
| Field | Type | Description |
|---|---|---|
| `total` | `u32` | Total calls attempted |
| `succeeded` | `u32` | Successful calls |
| `failed` | `u32` | Failed calls |
| `budget_exceeded_count` | `u32` | Failed calls that had a budget set |

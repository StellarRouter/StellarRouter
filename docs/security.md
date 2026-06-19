# Security Guide

This document covers the threat model for the stellar-router suite, known attack
vectors, and recommended mitigations for each.

## Threat Model

stellar-router is a set of on-chain Soroban contracts. The trust boundary is the
admin keypair. All sensitive mutations require admin authentication via
`require_auth()`. Off-chain components (api-server, metrics exporter) are
read-only or fire-and-forget and do not hold signing keys in production.

---

## Threats and Mitigations

### 1. Admin Key Compromise

**What can an attacker do?**
- Register malicious routes pointing to attacker-controlled contracts
- Drain the timelock queue by cancelling all pending operations
- Grant themselves arbitrary roles in router-access
- Disable middleware globally, bypassing rate limits and circuit breakers

**Mitigations**
- Use a hardware wallet or multi-sig for the admin keypair. Stellar supports
  multi-signature accounts natively — require M-of-N signatures for admin
  transactions.
- Rotate the admin key regularly using `transfer_admin` / `transfer_super_admin`.
  These functions emit `admin_transferred` events that off-chain monitors can alert on.
- Queue all sensitive admin operations through router-timelock with a minimum
  delay of at least 24 hours. This gives time to detect and cancel a compromised
  admin's actions before they execute.
- Monitor the `admin_transferred` event on-chain. An unexpected transfer is a
  strong signal of compromise.

---

### 2. Replay Attacks

**What can an attacker do?**
- Re-submit a previously valid signed transaction to repeat an operation
  (e.g., re-register a route that was removed, re-execute a timelock operation).

**Mitigations**
- Soroban transactions include a sequence number tied to the source account.
  The Stellar network rejects transactions with a sequence number that has
  already been used, providing native replay protection at the transaction level.
- For the api-server, the `replay_protection` middleware (in `metrics/src/replay_protection.rs`)
  implements nonce-based replay detection for HTTP requests. Enable it in
  production via `ROUTER_REPLAY_PROTECTION_ENABLED=true`.
- router-timelock operations are one-shot: `executed` and `cancelled` flags
  prevent re-execution at the contract level.

---

### 3. Rate Limit Bypass

**What can an attacker do?**
- Use many different caller addresses to bypass per-caller rate limits in
  router-middleware (Sybil attack).
- Call `pre_call` from a contract that rotates caller addresses to avoid
  the per-address window.

**Mitigations**
- Rate limits in router-middleware are per `(route, caller)` pair. For routes
  that require stricter controls, combine rate limiting with router-access role
  checks — only addresses holding a specific role can call `pre_call` at all.
- Set `failure_threshold` on the circuit breaker. Even if individual callers
  bypass per-caller limits, aggregate failures will trip the circuit and block
  all callers until the recovery window elapses.
- Consider setting a global rate limit (max total calls per window) in addition
  to per-caller limits for high-value routes.

---

### 4. Circuit Breaker Manipulation

**What can an attacker do?**
- Deliberately trigger failures to trip the circuit breaker and deny service
  to legitimate callers (griefing attack).
- Call `reset_circuit_breaker` (admin-only) to clear a legitimately tripped
  circuit and allow a compromised route to be called again.

**Mitigations**
- Set a recovery window (`recovery_window_seconds`) long enough that a griefing
  attacker cannot repeatedly trip and reset the circuit. A 5–15 minute window
  is a reasonable starting point.
- Monitor `circuit_opened` events. Repeated trips in a short window indicate
  either a griefing attack or a genuinely broken downstream contract.
- The `reset_circuit_breaker` function requires admin auth. Protect the admin
  key as described in threat #1.

---

### 5. Timelock Bypass via Fast-Track

**What can an attacker do?**
- If the emergency council is compromised (M-of-N members collude or are
  coerced), they can fast-track any operation immediately, bypassing `min_delay`.
- A single compromised council member cannot fast-track alone (requires M
  approvals), but can block legitimate fast-tracks by refusing to approve.

**Mitigations**
- Set M (required approvals) to at least ⌈N/2⌉ + 1 (strict majority) to
  require collusion of more than half the council.
- Keep the council list small (3–7 members) and geographically/organizationally
  distributed to reduce collusion risk.
- The council list itself can only be updated via a standard (non-fast-track)
  admin call — this means updating the council is subject to `min_delay`,
  preventing an attacker from adding themselves to the council and immediately
  fast-tracking.
- Disable fast-track (`set_fast_track_enabled(false)`) when no emergency is
  active. Re-enable only when needed.
- Monitor `critical_fast_tracked` events. Any fast-tracked execution should
  trigger an immediate review.

---

### 6. Route Hijacking

**What can an attacker do?**
- If an admin key is compromised, register a route with a legitimate name
  (e.g., "oracle") pointing to a malicious contract.
- Update an existing route to redirect traffic to an attacker-controlled address.

**Mitigations**
- All route mutations (`register_route`, `update_route`, `remove_route`) emit
  events. Monitor `route_registered`, `route_updated`, and `route_overwritten`
  events for unexpected changes.
- Queue route updates through router-timelock. This gives a delay window during
  which the update can be cancelled if it is unauthorized.
- Use router-registry to maintain a versioned record of legitimate contract
  addresses. Cross-reference resolved addresses against the registry before
  executing high-value operations.

---

### 7. Blacklist Bypass

**What can an attacker do?**
- A blacklisted address can still call contracts directly — the blacklist only
  prevents role grants in router-access. It does not prevent direct contract
  calls.

**Mitigations**
- The blacklist in router-access is a role-management control, not a firewall.
  Do not rely on it to prevent direct contract interactions.
- For routes that require strict caller control, use router-middleware with a
  role-based allowlist: only addresses holding a specific role can pass
  `pre_call`. Combine with router-access to manage that role.

---

## Security Checklist for Deployment

Before deploying to mainnet:

- [ ] Admin keypair is a hardware wallet or multi-sig account
- [ ] `min_delay` in router-timelock is set to at least 24 hours
- [ ] Emergency council is configured with M > N/2
- [ ] Fast-track is disabled by default (`set_fast_track_enabled(false)`)
- [ ] Replay protection is enabled in the api-server
- [ ] On-chain event monitoring is set up for `admin_transferred`,
      `route_updated`, `circuit_opened`, and `critical_fast_tracked`
- [ ] All contracts are initialized before any routes are registered
- [ ] The metrics exporter is running and dashboards are configured

---

## Role Hierarchy and Inheritance

The `router-access` contract supports a directed acyclic graph (DAG) of roles. This allows for complex permission structures where higher-level roles automatically inherit the permissions of lower-level roles.

### Example: Admin → Editor → Viewer

A common setup is a three-tier hierarchy:
1.  **Viewer**: Can read data.
2.  **Editor**: Can read and modify data.
3.  **Admin**: Can read, modify, and manage permissions.

To set this up using `router-access`:

```bash
# 1. Set Editor as the parent of Viewer (Editor → Viewer)
# Anyone with 'editor' role now implicitly has 'viewer' role
stellar contract invoke --id <ACCESS_ID> -- \
  set_role_parent --caller <SUPER_ADMIN> --role "viewer" --parent_role "editor"

# 2. Set Admin as the parent of Editor (Admin → Editor)
# Anyone with 'admin' role now implicitly has 'editor' AND 'viewer' roles
stellar contract invoke --id <ACCESS_ID> -- \
  set_role_parent --caller <SUPER_ADMIN> --role "editor" --parent_role "admin"

# 3. Grant 'admin' to a user
stellar contract invoke --id <ACCESS_ID> -- \
  grant_role --admin <SUPER_ADMIN> --account <USER_ADDRESS> --role "admin"
```

### Inheritance Resolution

Inheritance is resolved at check-time by walking up the ancestor chain:
- `has_role(user, "viewer")` checks if the user has `viewer`, `editor`, or `admin` directly.
- The check stops at a maximum depth of 16 to prevent gas exhaustion from deep hierarchies.
- **Transitivity**: If A is parent of B, and B is parent of C, then A is implicitly a parent of C.

### Common Pitfalls

- **Cycles**: The contract prevents cycles (e.g., A → B → A) during `set_role_parent`. Attempting to create one returns `HierarchyCycle`.
- **Blacklisting**: Blacklisting an address overrides all inherited roles. If an address is blacklisted, `has_role` returns `false` regardless of hierarchy.
- **Revocation**: `revoke_role` only removes a **direct** grant. If a user has `admin` and you revoke `viewer`, they still have `viewer` because it is inherited from `admin`. To fully remove access, you must revoke the highest-level role in their chain.
- **Super-Admin Bypass**: The `super_admin` does **not** implicitly hold all roles. They must grant themselves roles or be added to the hierarchy if they need to pass `has_role` checks in other contracts.

---

## Reporting Security Issues

If you discover a security vulnerability in stellar-router, please do **not**
open a public GitHub issue. Instead, contact the maintainers directly via the
contact information in the repository's security policy.

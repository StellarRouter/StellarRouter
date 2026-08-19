# Event Naming Convention

All smart contracts in the stellar-router suite follow a consistent event naming pattern to ensure predictable integration and monitoring.

## Convention Rules

1. **Use past tense verbs** for events (action completed)
   - ✓ `admin_transferred` — admin was transferred
   - ✓ `role_granted` — role was granted
   - ✓ `route_registered` — route was registered
   - ✗ `grant_role` — ambiguous (command vs event)
   - ✗ `role_grant` — incomplete verb form

2. **Use snake_case** for all event names
   - ✓ `metadata_updated`
   - ✗ `metadataUpdated`

3. **Use action + subject order** when possible
   - ✓ `admin_transferred` (what + who)
   - ✓ `route_registered` (what + what)
   - ✓ `role_revoked` (what + what)

4. **Event payloads should include relevant context**
   - Include the target of the action (e.g., old and new admin for `admin_transferred`)
   - Include identifiers needed to track the action (e.g., route name, role, address)

## Examples by Contract

### router-core
- `route_registered` — (route_name, address)
- `routed` — (route_name, address)
- `alias_added` — (existing_name, alias_name)
- `metadata_updated` — (route_name, metadata)
- `admin_transferred` — (old_admin, new_admin)

### router-registry
- `contract_registered` — (contract_name, version)
- `contract_deprecated` — (contract_name, version)
- `admin_transferred` — (old_admin, new_admin)

### router-access
- `role_granted` — (role, target, expiry_timestamp)
- `role_revoked` — (role, target)
- `role_parent_set` — (role, parent_role)
- `role_parent_removed` — (role, parent_role)
- `role_admin_set` — (role, admin)
- `address_blacklisted` — (address)
- `address_unblacklisted` — (address)
- `role_expired` — (role, target)
- `admin_transferred` — (old_admin, new_admin)

### router-timelock
- `op_queued` — (op_id, target, eta)
- `op_executed` — (op_id, target)
- `op_cancelled` — (op_id)

### router-multicall
- `call_result` — (caller, target, function, success)
- `batch_executed` — (summary data)
- `max_batch_size_updated` — (old_size, new_size)
- `admin_transferred` — (old_admin, new_admin)

### router-middleware
- `rate_limit_exceeded` — (caller, route)
- `call_logged` — (caller, route, timestamp, success)
- `middleware_enabled` — (enabled)
- `route_config_updated` — (route_name, config)
- `circuit_breaker_opened` — (route_name)
- `admin_transferred` — (old_admin, new_admin)

### router-execution
- `execution_result` — (target, function, success, attempts)
- `fee_estimated` — (total_fee, surge_pricing)
- `simulation_result` — (target, function, success)

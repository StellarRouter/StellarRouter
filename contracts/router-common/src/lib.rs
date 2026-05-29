//! # router-common
//!
//! Shared macros and utilities for the stellar-router suite.
//!
//! ## Macros
//! - [`require_admin!`] — inline admin check used across router contracts
//! - [`require_admin_simple!`] — convenience macro for standard DataKey::Admin and error variants
//! - [`admin_transfer_complete!`] — shared admin transfer pattern (storage set + event emit)
//!
//! ## Event Topic Naming Convention
//!
//! All event topics across stellar-router contracts follow these rules:
//!
//! 1. **Use snake_case** - All event topics must use lowercase with underscores
//!    - ✅ Good: `route_registered`, `admin_transferred`, `role_granted`
//!    - ❌ Bad: `routeRegistered`, `AdminTransferred`, `RoleGranted`
//!
//! 2. **Use descriptive past-tense verbs** - Events represent actions that have occurred
//!    - ✅ Good: `route_registered`, `role_revoked`, `circuit_opened`
//!    - ❌ Bad: `register_route`, `revoke_role`, `open_circuit`
//!
//! 3. **Be specific and unambiguous** - Event names should clearly indicate what happened
//!    - ✅ Good: `max_batch_size_updated`, `route_resolve_paused`
//!    - ❌ Bad: `updated`, `paused`
//!
//! 4. **Use full words, avoid abbreviations** - Clarity over brevity
//!    - ✅ Good: `execution_result`, `simulation_result`
//!    - ❌ Bad: `exec_result`, `sim_result`
//!
//! 5. **Consistent terminology** - Use the same terms across related events
//!    - Admin events: `admin_transferred`
//!    - Role events: `role_granted`, `role_revoked`, `role_parent_set`
//!    - Route events: `route_registered`, `route_updated`, `route_overwritten`
//!
//! ## Standard Event Topics
//!
//! Use these constants when publishing events to ensure consistency:

/// Standard event topic for admin transfer operations
pub const EVENT_ADMIN_TRANSFERRED: &str = "admin_transferred";

/// Standard event topic for route registration
pub const EVENT_ROUTE_REGISTERED: &str = "route_registered";

/// Standard event topic for route updates
pub const EVENT_ROUTE_UPDATED: &str = "route_updated";

/// Standard event topic for route overwrites
pub const EVENT_ROUTE_OVERWRITTEN: &str = "route_overwritten";

/// Standard event topic for paused route resolution attempts
pub const EVENT_ROUTE_RESOLVE_PAUSED: &str = "route_resolve_paused";

/// Standard event topic for successful routing
pub const EVENT_ROUTED: &str = "routed";

/// Standard event topic for route scoring
pub const EVENT_ROUTE_SCORED: &str = "route_scored";

/// Standard event topic for best route selection
pub const EVENT_BEST_ROUTE_SELECTED: &str = "best_route_selected";

/// Standard event topic for metadata updates
pub const EVENT_METADATA_UPDATED: &str = "metadata_updated";

/// Standard event topic for alias additions
pub const EVENT_ALIAS_ADDED: &str = "alias_added";

/// Standard event topic for role grants
pub const EVENT_ROLE_GRANTED: &str = "role_granted";

/// Standard event topic for role revocations
pub const EVENT_ROLE_REVOKED: &str = "role_revoked";

/// Standard event topic for role parent assignments
pub const EVENT_ROLE_PARENT_SET: &str = "role_parent_set";

/// Standard event topic for role parent removals
pub const EVENT_ROLE_PARENT_REMOVED: &str = "role_parent_removed";

/// Standard event topic for address blacklisting
pub const EVENT_ADDRESS_BLACKLISTED: &str = "address_blacklisted";

/// Standard event topic for execution results
pub const EVENT_EXECUTION_RESULT: &str = "execution_result";

/// Standard event topic for execution retries
pub const EVENT_EXECUTION_RETRY: &str = "execution_retry";

/// Standard event topic for execution errors
pub const EVENT_EXECUTION_ERROR: &str = "execution_error";

/// Standard event topic for simulation results
pub const EVENT_SIMULATION_RESULT: &str = "simulation_result";

/// Standard event topic for fee estimations
pub const EVENT_FEE_ESTIMATED: &str = "fee_estimated";

/// Standard event topic for quote generation
pub const EVENT_QUOTE_GENERATED: &str = "quote_generated";

/// Standard event topic for pre-call middleware hooks
pub const EVENT_PRE_CALL: &str = "pre_call";

/// Standard event topic for post-call middleware hooks
pub const EVENT_POST_CALL: &str = "post_call";

/// Standard event topic for circuit breaker opening
pub const EVENT_CIRCUIT_OPENED: &str = "circuit_opened";

/// Standard event topic for call log clearing
pub const EVENT_CALL_LOG_CLEARED: &str = "call_log_cleared";

/// Standard event topic for multicall results
pub const EVENT_CALL_RESULT: &str = "call_result";

/// Standard event topic for max batch size updates
pub const EVENT_MAX_BATCH_SIZE_UPDATED: &str = "max_batch_size_updated";

/// Standard event topic for timelock operation queueing
pub const EVENT_OP_QUEUED: &str = "op_queued";

/// Standard event topic for contract registration in registry
pub const EVENT_CONTRACT_REGISTERED: &str = "contract_registered";

/// Standard event topic for contract deprecation in registry
pub const EVENT_CONTRACT_DEPRECATED: &str = "contract_deprecated";

/// Checks that `caller` matches the admin address stored under `key`.
///
/// Expands to an expression that returns `Err($not_init_err)` if the key is
/// absent, or `Err($unauth_err)` if the caller does not match.
///
/// # Arguments
/// * `$env`          — `&Env` reference
/// * `$caller`       — `&Address` to validate
/// * `$key`          — storage key whose value is the admin `Address`
/// * `$not_init_err` — error variant returned when the key is missing
/// * `$unauth_err`   — error variant returned when the caller is not the admin
///
/// # Example
///
/// ```ignore
/// // Inside a #[contractimpl] block:
/// require_admin!(&env, &caller, &DataKey::Admin, MyError::NotInitialized, MyError::Unauthorized)?;
/// ```
#[macro_export]
macro_rules! require_admin {
    ($env:expr, $caller:expr, $key:expr, $not_init_err:expr, $unauth_err:expr) => {{
        let admin: soroban_sdk::Address = $env
            .storage()
            .instance()
            .get($key)
            .ok_or($not_init_err)?;
        if &admin != $caller {
            return Err($unauth_err);
        }
        Ok::<(), _>(())
    }};
}

/// Returns `true` if `s` is empty or consists entirely of ASCII whitespace
/// (space 0x20, tab 0x09, newline 0x0A, vertical tab 0x0B, form feed 0x0C,
/// carriage return 0x0D).
///
/// # Example
///
/// ```
/// use router_common::is_whitespace_only;
/// assert!(is_whitespace_only(""));
/// assert!(is_whitespace_only("   "));
/// assert!(is_whitespace_only("\t\n\r"));
/// assert!(!is_whitespace_only("oracle"));
/// assert!(!is_whitespace_only(" oracle "));
/// ```
pub fn is_whitespace_only(s: &str) -> bool {
    s.is_empty() || s.bytes().all(|b| matches!(b, 9 | 10 | 11 | 12 | 13 | 32))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_empty_string_is_whitespace_only() {
        assert!(is_whitespace_only(""));
    }

    #[test]
    fn test_spaces_are_whitespace_only() {
        assert!(is_whitespace_only("   "));
    }

    #[test]
    fn test_tab_is_whitespace_only() {
        assert!(is_whitespace_only("\t"));
    }

    #[test]
    fn test_newline_is_whitespace_only() {
        assert!(is_whitespace_only("\n"));
    }

    #[test]
    fn test_carriage_return_is_whitespace_only() {
        assert!(is_whitespace_only("\r"));
    }

    #[test]
    fn test_mixed_whitespace_is_whitespace_only() {
        assert!(is_whitespace_only(" \t\n\r\x0b\x0c"));
    }

    #[test]
    fn test_normal_name_is_not_whitespace_only() {
        assert!(!is_whitespace_only("oracle"));
    }

    #[test]
    fn test_name_with_surrounding_spaces_is_not_whitespace_only() {
        assert!(!is_whitespace_only(" oracle "));
    }
}

/// Convenience version when using DataKey::Admin and standard error variants
#[macro_export]
macro_rules! require_admin_simple {
    ($env:expr, $caller:expr, $data_key:expr, $error_type:ty) => {
        $crate::require_admin!(
            $env,
            $caller,
            $data_key,
            <$error_type>::NotInitialized,
            <$error_type>::Unauthorized
        )
    };
}

/// Helper macro for completing the admin transfer after validation.
///
/// Use this in your transfer_admin function after you've already:
/// - Called current.require_auth()
/// - Called your own require_admin check
///
/// This macro:
/// - Sets the new admin in storage
/// - Publishes the admin_transferred event using the standard event topic
///
/// # Arguments
/// * `$env` - The Soroban environment reference
/// * `$current` - The current admin address (Address)
/// * `$new_admin` - The new admin address (Address)
/// * `$data_key_expr` - Expression for the storage key containing admin (e.g., &DataKey::Admin)
///
/// # Example
/// ```ignore
/// pub fn transfer_admin(
///     env: Env,
///     current: Address,
///     new_admin: Address,
/// ) -> Result<(), MyError> {
///     current.require_auth();
///     router_common::require_admin_simple!(&env, &current, &DataKey::Admin, MyError)?;
///     router_common::admin_transfer_complete!(&env, &current, &new_admin, &DataKey::Admin);
///     Ok(())
/// }
/// ```
#[macro_export]
macro_rules! admin_transfer_complete {
    ($env:expr, $current:expr, $new_admin:expr, $data_key_expr:expr) => {
        {
            $env.storage().instance().set($data_key_expr, $new_admin);
            $env.events().publish(
                (soroban_sdk::Symbol::new($env, $crate::EVENT_ADMIN_TRANSFERRED),),
                ($current, $new_admin),
            );
        }
    };
}

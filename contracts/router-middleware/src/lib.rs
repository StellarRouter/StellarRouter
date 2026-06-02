#![no_std]

//! # router-middleware
//!
//! Pre/post call hook middleware for the stellar-router suite.
//! Supports rate limiting, call logging, and per-route fee configuration.
//!
//! ## Features
//! - Per-caller rate limiting (max calls per time window)
//! - Call event logging with timestamps
//! - Configurable per-route fees
//! - Admin-controlled hook enable/disable
//!
//! ## Events (following naming convention: past tense verbs in snake_case)
//! - `pre_call` — Pre-call validation hook executed
//! - `post_call` — Post-call hook executed
//! - `circuit_opened` — Circuit breaker opened for route
//! - `middleware_enabled` — Global middleware enabled/disabled
//! - `call_log_cleared` — Call log cleared for route
//! - `admin_transferred` — Admin transferred to new address

use soroban_sdk::{
    contract, contracterror, contractimpl, contracttype, Address, Env, Map, String, Symbol, Vec,
};

// ── Storage Keys ──────────────────────────────────────────────────────────────

#[contracttype]
pub enum DataKey {
    Admin,
    RouteCallState(String), // route_name -> RouteCallState
    RouteConfig(String),        // route_name -> RouteConfig
    GlobalEnabled,
    TotalCalls,
    CallLog(String),        // route_name -> CallLogState
    ConfiguredRoutes,       // Vec<String>
    CallLogSummary(String), // route_name -> CallLogSummary
}

// ── Types ─────────────────────────────────────────────────────────────────────

#[contracttype]
#[derive(Clone, Debug, PartialEq)]
pub struct RateLimitState {
    /// Number of calls in current window
    pub calls_in_window: u32,
    /// Timestamp when window started
    pub window_start: u64,
    /// Total number of times rate limit was exceeded
    pub total_violations: u32,
}

#[contracttype]
#[derive(Clone, Debug, PartialEq)]
pub struct RouteRateLimitStats {
    /// Total number of calls across all callers in current window
    pub total_calls_in_window: u32,
    /// Timestamp when the current window started (earliest window start among all callers)
    pub window_start: u64,
    /// Total number of rate limit violations across all callers
    pub total_violations: u32,
}

#[contracttype]
#[derive(Clone, Debug, PartialEq)]
pub struct RouteConfig {
    /// Max calls per window (0 = unlimited)
    pub max_calls_per_window: u32,
    /// Window size in seconds
    pub window_seconds: u64,
    /// Whether this route is enabled
    pub enabled: bool,
    /// Circuit breaker failure threshold (0 = disabled)
    pub failure_threshold: u32,
    /// Circuit breaker recovery window in seconds
    pub recovery_window_seconds: u64,
    /// Max call log entries to keep (0 = disabled)
    pub log_retention: u32,
}

#[contracttype]
#[derive(Clone, Debug, PartialEq)]
pub struct CircuitBreakerState {
    /// Number of consecutive failures
    pub failure_count: u32,
    /// Timestamp when circuit was opened
    pub opened_at: u64,
    /// Whether circuit is currently open
    pub is_open: bool,
    /// Whether circuit is in half-open state (probe mode)
    pub is_half_open: bool,
}

#[contracttype]
#[derive(Clone, Debug, PartialEq)]
pub struct RouteCallState {
    /// Per-caller rate limit state for the route
    pub rate_limits: Map<Address, RateLimitState>,
    /// Route-level circuit breaker state
    pub circuit_breaker: CircuitBreakerState,
}

#[contracttype]
#[derive(Clone, Debug, PartialEq)]
pub struct CallLogEntry {
    /// The caller address
    pub caller: Address,
    /// Timestamp of the call
    pub timestamp: u64,
    /// Whether the call succeeded
    pub success: bool,
    /// The route that was called
    pub route: String,
}

#[contracttype]
#[derive(Clone, Debug, PartialEq)]
pub struct CallLogState {
    /// Fixed-capacity call entries retained for the route
    pub entries: Vec<CallLogEntry>,
    /// Index of the oldest entry in `entries` (0 when not wrapped)
    pub head: u32,
}

/// Aggregated summary for a route's call log.
/// Maintained incrementally to avoid loading all entries.
#[contracttype]
#[derive(Clone, Debug, PartialEq)]
pub struct CallLogSummary {
    pub total_calls: u32,
    pub success_count: u32,
    pub failure_count: u32,
    pub last_call_timestamp: u64,
}

// ── Errors ────────────────────────────────────────────────────────────────────

#[contracterror]
#[derive(Copy, Clone, Debug, PartialEq)]
pub enum MiddlewareError {
    AlreadyInitialized = 1,
    NotInitialized = 2,
    Unauthorized = 3,
    RateLimitExceeded = 4,
    RouteDisabled = 5,
    MiddlewareDisabled = 6,
    InvalidConfig = 7,
    CircuitOpen = 8,
}

// ── Contract ──────────────────────────────────────────────────────────────────

#[contract]
pub struct RouterMiddleware;

#[contractimpl]
impl RouterMiddleware {
    /// Initialize middleware with an admin.
    ///
    /// Must be called exactly once. Sets the admin, enables middleware globally,
    /// and resets the total call counter to zero.
    ///
    /// # Arguments
    /// * `env` - The Soroban environment.
    /// * `admin` - The address that will have admin privileges over this middleware.
    ///
    /// # Returns
    /// `Ok(())` on success.
    ///
    /// # Errors
    /// * [`MiddlewareError::AlreadyInitialized`] — if the contract has already been initialized.
    pub fn initialize(env: Env, admin: Address) -> Result<(), MiddlewareError> {
        if env.storage().instance().has(&DataKey::Admin) {
            return Err(MiddlewareError::AlreadyInitialized);
        }
        env.storage().instance().set(&DataKey::Admin, &admin);
        env.storage().instance().set(&DataKey::GlobalEnabled, &true);
        env.storage().instance().set(&DataKey::TotalCalls, &0u64);
        Ok(())
    }

    /// Configure a route's middleware settings.
    ///
    /// Sets the rate-limit window and call cap for `route`, and whether the
    /// route is enabled. If `max_calls_per_window` is 0, rate limiting is
    /// disabled for that route. Caller must be the admin.
    ///
    /// # Arguments
    /// * `env` - The Soroban environment.
    /// * `caller` - The address initiating the call; must be the admin.
    /// * `route` - The route name to configure.
    /// * `max_calls_per_window` - Maximum allowed calls per time window (0 = unlimited).
    /// * `window_seconds` - Duration of the rate-limit window in seconds.
    /// * `enabled` - Whether this route should be enabled.
    /// * `failure_threshold` - Circuit breaker failure threshold (0 = disabled).
    /// * `recovery_window_seconds` - Circuit breaker recovery window in seconds.
    /// * `log_retention` - Maximum call log entries to keep (0 = disabled).
    ///
    /// # Returns
    /// `Ok(())` on success.
    ///
    /// # Errors
    /// * [`MiddlewareError::Unauthorized`] — if `caller` is not the admin.
    /// * [`MiddlewareError::InvalidConfig`] — if `window_seconds` is 0 while `max_calls_per_window` > 0.
    /// * [`MiddlewareError::NotInitialized`] — if the contract has not been initialized.
    pub fn configure_route(
        env: Env,
        caller: Address,
        route: String,
        max_calls_per_window: u32,
        window_seconds: u64,
        enabled: bool,
        failure_threshold: u32,
        recovery_window_seconds: u64,
        log_retention: u32,
    ) -> Result<(), MiddlewareError> {
        caller.require_auth();
        router_common::require_admin_simple!(&env, &caller, &DataKey::Admin, MiddlewareError)?;

        if window_seconds == 0 && max_calls_per_window > 0 {
            return Err(MiddlewareError::InvalidConfig);
        }

        let config = RouteConfig {
            max_calls_per_window,
            window_seconds,
            enabled,
            failure_threshold,
            recovery_window_seconds,
            log_retention,
        };
        env.storage()
            .instance()
            .set(&DataKey::RouteConfig(route.clone()), &config);

        let mut configured: Vec<String> = env
            .storage()
            .instance()
            .get(&DataKey::ConfiguredRoutes)
            .unwrap_or_else(|| Vec::new(&env));
        if !configured.contains(&route) {
            configured.push_back(route.clone());
            env.storage()
                .instance()
                .set(&DataKey::ConfiguredRoutes, &configured);
        }

        Ok(())
    }

    /// Pre-call hook: validates rate limits and route status.
    ///
    /// Must be called before routing to a contract. Checks that middleware is
    /// globally enabled, that the specific route is enabled, and that the
    /// `caller` has not exceeded their rate limit for `route`. All validation
    /// is performed before any state is written — if any check fails, no state
    /// is modified. On success, increments the global call counter, updates the
    /// rate limit state, and emits a `pre_call` event.
    ///
    /// # Arguments
    /// * `env` - The Soroban environment.
    /// * `caller` - The address making the routed call.
    /// * `route` - The name of the route being called.
    ///
    /// # Returns
    /// `Ok(())` if the call is allowed to proceed.
    ///
    /// # Errors
    /// * [`MiddlewareError::MiddlewareDisabled`] — if middleware is globally disabled.
    /// * [`MiddlewareError::RouteDisabled`] — if the specific route is disabled.
    /// * [`MiddlewareError::RateLimitExceeded`] — if `caller` has exceeded the rate limit for `route`.
    /// * [`MiddlewareError::CircuitOpen`] — if the circuit breaker is open for the route.
    pub fn pre_call(env: Env, caller: Address, route: String) -> Result<(), MiddlewareError> {
        // ── Validation phase (no state writes) ───────────────────────────────

        // 1. Check global enable
        let enabled: bool = env
            .storage()
            .instance()
            .get(&DataKey::GlobalEnabled)
            .unwrap_or(true);
        if !enabled {
            return Err(MiddlewareError::MiddlewareDisabled);
        }

        // 2. Compute new states (if applicable) without writing yet
        let new_route_call_state = if let Some(config) =
            env.storage()
                .instance()
                .get::<DataKey, RouteConfig>(&DataKey::RouteConfig(route.clone()))
        {
            let mut route_call_state: RouteCallState = env
                .storage()
                .instance()
                .get(&DataKey::RouteCallState(route.clone()))
                .unwrap_or(RouteCallState {
                    rate_limits: Map::new(&env),
                    circuit_breaker: CircuitBreakerState {
                        failure_count: 0,
                        opened_at: 0,
                        is_open: false,
                        is_half_open: false,
                    },
                });

            // 2a. Route enabled check
            if !config.enabled {
                return Err(MiddlewareError::RouteDisabled);
            }

            // 2b. Circuit breaker check
            let mut state_changed = false;
            if config.failure_threshold > 0 {
                if route_call_state.circuit_breaker.is_open {
                    let now = env.ledger().timestamp();
                    let recovers = config.recovery_window_seconds > 0
                        && now
                            >= route_call_state.circuit_breaker.opened_at
                                + config.recovery_window_seconds;
                    if !recovers {
                        return Err(MiddlewareError::CircuitOpen);
                    }
                    // Transition to half-open state for probe call
                    route_call_state.circuit_breaker.is_open = false;
                    route_call_state.circuit_breaker.is_half_open = true;
                    state_changed = true;
                } else if route_call_state.circuit_breaker.is_half_open {
                    // Already in half-open state - allow this probe call
                    // The state will be updated in post_call based on success/failure
                }
            }

            // 2c. Rate limit check — compute new state but do not write yet
            if config.max_calls_per_window > 0 {
                let now = env.ledger().timestamp();
                let state: RateLimitState =
                    route_call_state
                        .rate_limits
                        .get(caller.clone())
                        .unwrap_or(RateLimitState {
                            calls_in_window: 0,
                            window_start: now,
                            total_violations: 0,
                        });

                let window_elapsed = now >= state.window_start + config.window_seconds;
                let calls = if window_elapsed {
                    0
                } else {
                    state.calls_in_window
                };
                let window_start = if window_elapsed {
                    now
                } else {
                    state.window_start
                };

                if calls >= config.max_calls_per_window {
                    // Increment violation counter before returning error
                    route_call_state.rate_limits.set(
                        caller.clone(),
                        RateLimitState {
                            calls_in_window: calls,
                            window_start,
                            total_violations: state.total_violations + 1,
                        },
                    );
                    env.storage()
                        .instance()
                        .set(&DataKey::RouteCallState(route.clone()), &route_call_state);
                    return Err(MiddlewareError::RateLimitExceeded);
                }

                route_call_state.rate_limits.set(
                    caller.clone(),
                    RateLimitState {
                        calls_in_window: calls + 1,
                        window_start,
                        total_violations: state.total_violations,
                    },
                );
                state_changed = true;
            }

            if state_changed {
                Some(route_call_state)
            } else {
                None
            }
        } else {
            None
        };

        // ── Commit phase (all checks passed — write state atomically) ─────────

        // Re-check global and route enabled flags immediately before committing
        // to close the window between validation and write.
        let still_enabled: bool = env
            .storage()
            .instance()
            .get(&DataKey::GlobalEnabled)
            .unwrap_or(true);
        if !still_enabled {
            return Err(MiddlewareError::MiddlewareDisabled);
        }

        if let Some(config) = env
            .storage()
            .instance()
            .get::<DataKey, RouteConfig>(&DataKey::RouteConfig(route.clone()))
        {
            if !config.enabled {
                return Err(MiddlewareError::RouteDisabled);
            }
        }

        // Write combined route call state once (rate limit + circuit breaker)
        if let Some(route_call_state) = new_route_call_state {
            env.storage()
                .instance()
                .set(&DataKey::RouteCallState(route.clone()), &route_call_state);
        }

        // Increment global call counter
        let total: u64 = env
            .storage()
            .instance()
            .get(&DataKey::TotalCalls)
            .unwrap_or(0);
        env.storage()
            .instance()
            .set(&DataKey::TotalCalls, &(total + 1));

        // Emit call event
        env.events().publish(
            (Symbol::new(&env, "pre_call"),),
            (caller.clone(), route.clone()),
        );

        Ok(())
    }

    /// Post-call hook: tracks failures and manages circuit breaker.
    ///
    /// Should be called after a routed contract call completes. Emits a
    /// `post_call` event with the caller, route name, and outcome. If the call
    /// failed and the route has a circuit breaker configured, increments the
    /// failure count and trips the circuit if the threshold is reached.
    ///
    /// # Arguments
    /// * `env` - The Soroban environment.
    /// * `caller` - The address that made the routed call.
    /// * `route` - The name of the route that was called.
    /// * `success` - `true` if the call succeeded, `false` if it failed.
    pub fn post_call(env: Env, caller: Address, route: String, success: bool) {
        env.events().publish(
            (Symbol::new(&env, "post_call"),),
            (caller.clone(), route.clone(), success),
        );

        // Log the call if retention is enabled
        if let Some(config) = env
            .storage()
            .instance()
            .get::<DataKey, RouteConfig>(&DataKey::RouteConfig(route.clone()))
        {
            if config.log_retention > 0 {
                let mut log: CallLogState = env
                    .storage()
                    .instance()
                    .get(&DataKey::CallLog(route.clone()))
                    .unwrap_or(CallLogState {
                        entries: Vec::new(&env),
                        head: 0,
                    });

                let entry = CallLogEntry {
                    caller: caller.clone(),
                    timestamp: env.ledger().timestamp(),
                    success,
                    route: route.clone(),
                };

                let cap = config.log_retention;
                if log.entries.len() < cap {
                    log.entries.push_back(entry);
                } else if cap > 0 {
                    // Overwrite oldest slot and advance head (fixed-size ring buffer)
                    log.entries.set(log.head, entry);
                    log.head = (log.head + 1) % cap;
                }

                env.storage()
                    .instance()
                    .set(&DataKey::CallLog(route.clone()), &log);

                // Update summary incrementally (avoids reloading all entries)
                let mut summary: CallLogSummary = env
                    .storage()
                    .instance()
                    .get(&DataKey::CallLogSummary(route.clone()))
                    .unwrap_or(CallLogSummary {
                        total_calls: 0,
                        success_count: 0,
                        failure_count: 0,
                        last_call_timestamp: 0,
                    });
                summary.total_calls += 1;
                if success {
                    summary.success_count += 1;
                } else {
                    summary.failure_count += 1;
                }
                summary.last_call_timestamp = env.ledger().timestamp();
                env.storage()
                    .instance()
                    .set(&DataKey::CallLogSummary(route.clone()), &summary);
            }
        }

        if !success {
            if let Some(config) = env
                .storage()
                .instance()
                .get::<DataKey, RouteConfig>(&DataKey::RouteConfig(route.clone()))
            {
                if config.failure_threshold > 0 {
                    let mut route_call_state: RouteCallState = env
                        .storage()
                        .instance()
                        .get(&DataKey::RouteCallState(route.clone()))
                        .unwrap_or(RouteCallState {
                            rate_limits: Map::new(&env),
                            circuit_breaker: CircuitBreakerState {
                                failure_count: 0,
                                opened_at: 0,
                                is_open: false,
                                is_half_open: false,
                            },
                        });

                    // Handle half-open state: if probe fails, reopen circuit
                    if route_call_state.circuit_breaker.is_half_open {
                        route_call_state.circuit_breaker.is_half_open = false;
                        route_call_state.circuit_breaker.is_open = true;
                        route_call_state.circuit_breaker.opened_at = env.ledger().timestamp();
                        route_call_state.circuit_breaker.failure_count = 1;
                        env.events().publish(
                            (Symbol::new(&env, "circuit_opened"),),
                            (route.clone(), route_call_state.circuit_breaker.failure_count),
                        );
                    } else {
                        // Normal failure handling
                        route_call_state.circuit_breaker.failure_count += 1;

                        if route_call_state.circuit_breaker.failure_count >= config.failure_threshold {
                            route_call_state.circuit_breaker.is_open = true;
                            route_call_state.circuit_breaker.opened_at = env.ledger().timestamp();
                            env.events().publish(
                                (Symbol::new(&env, "circuit_opened"),),
                                (route.clone(), route_call_state.circuit_breaker.failure_count),
                            );
                        }
                    }

                    env.storage()
                        .instance()
                        .set(&DataKey::RouteCallState(route), &route_call_state);
                }
            }
        } else {
            if let Some(config) = env
                .storage()
                .instance()
                .get::<DataKey, RouteConfig>(&DataKey::RouteConfig(route.clone()))
            {
                if config.failure_threshold > 0 {
                    let mut route_call_state: RouteCallState = env
                        .storage()
                        .instance()
                        .get(&DataKey::RouteCallState(route.clone()))
                        .unwrap_or(RouteCallState {
                            rate_limits: Map::new(&env),
                            circuit_breaker: CircuitBreakerState {
                                failure_count: 0,
                                opened_at: 0,
                                is_open: false,
                                is_half_open: false,
                            },
                        });

                    // Handle half-open state: if probe succeeds, close circuit
                    if route_call_state.circuit_breaker.is_half_open {
                        route_call_state.circuit_breaker.is_half_open = false;
                        route_call_state.circuit_breaker.failure_count = 0;
                    } else if !route_call_state.circuit_breaker.is_open
                        && route_call_state.circuit_breaker.failure_count > 0
                    {
                        route_call_state.circuit_breaker.failure_count = 0;
                    }

                    env.storage()
                        .instance()
                        .set(&DataKey::RouteCallState(route), &route_call_state);
                }
            }
        }
    }

    /// Enable or disable all middleware globally.
    ///
    /// When disabled, `pre_call` will return
    /// [`MiddlewareError::MiddlewareDisabled`] for every route. Caller must be
    /// the admin.
    ///
    /// # Arguments
    /// * `env` - The Soroban environment.
    /// * `caller` - The address initiating the call; must be the admin.
    /// * `enabled` - `true` to enable middleware, `false` to disable it.
    ///
    /// # Returns
    /// `Ok(())` on success.
    ///
    /// # Errors
    /// * [`MiddlewareError::Unauthorized`] — if `caller` is not the admin.
    /// * [`MiddlewareError::NotInitialized`] — if the contract has not been initialized.
    pub fn set_global_enabled(
        env: Env,
        caller: Address,
        enabled: bool,
    ) -> Result<(), MiddlewareError> {
        caller.require_auth();
        router_common::require_admin_simple!(&env, &caller, &DataKey::Admin, MiddlewareError)?;
        env.storage()
            .instance()
            .set(&DataKey::GlobalEnabled, &enabled);
        env.events()
            .publish((Symbol::new(&env, "middleware_enabled"),), enabled);
        Ok(())
    }

    /// Get total calls processed.
    ///
    /// Returns the cumulative count of calls that have passed through
    /// `pre_call` since the contract was initialized.
    ///
    /// # Arguments
    /// * `env` - The Soroban environment.
    ///
    /// # Returns
    /// The total number of pre-call invocations.
    pub fn total_calls(env: Env) -> u64 {
        env.storage()
            .instance()
            .get(&DataKey::TotalCalls)
            .unwrap_or(0)
    }
    /// Get the call log for a route.
    ///
    /// Returns the list of recent call log entries for `route`, up to the
    /// configured retention limit. Entries are in chronological order (oldest first).
    ///
    /// # Arguments
    /// * `env` - The Soroban environment.
    /// * `route` - The route name to retrieve logs for.
    ///
    /// # Returns
    /// A [`Vec<CallLogEntry>`] of call log entries.
    pub fn get_call_log(env: Env, route: String) -> Vec<CallLogEntry> {
        let Some(log_state) = env
            .storage()
            .instance()
            .get::<DataKey, CallLogState>(&DataKey::CallLog(route))
        else {
            return Vec::new(&env);
        };

        if log_state.entries.is_empty() || log_state.head == 0 {
            return log_state.entries;
        }

        let len = log_state.entries.len();
        let mut ordered = Vec::new(&env);
        for i in 0..len {
            let idx = (log_state.head + i) % len;
            if let Some(entry) = log_state.entries.get(idx) {
                ordered.push_back(entry);
            }
        }
        ordered
    }

    /// Get a filtered call log for a route.
    ///
    /// Returns only successful or only failed call log entries for `route`,
    /// reducing data transfer for monitoring use cases compared to `get_call_log`.
    /// Entries are returned in chronological order (oldest first).
    ///
    /// # Arguments
    /// * `env` - The Soroban environment.
    /// * `route` - The route name to retrieve logs for.
    /// * `success_only` - If `true`, returns only successful entries; if `false`, returns only failed entries.
    ///
    /// # Returns
    /// A [`Vec<CallLogEntry>`] containing only entries matching the filter.
    pub fn get_call_log_filtered(env: Env, route: String, success_only: bool) -> Vec<CallLogEntry> {
        let Some(log_state) = env
            .storage()
            .instance()
            .get::<DataKey, CallLogState>(&DataKey::CallLog(route))
        else {
            return Vec::new(&env);
        };

        if log_state.entries.is_empty() {
            return Vec::new(&env);
        }

        let len = log_state.entries.len();
        let mut ordered = Vec::new(&env);

        if log_state.head == 0 {
            for i in 0..len {
                if let Some(entry) = log_state.entries.get(i) {
                    if entry.success == success_only {
                        ordered.push_back(entry);
                    }
                }
            }
        } else {
            for i in 0..len {
                let idx = (log_state.head + i) % len;
                if let Some(entry) = log_state.entries.get(idx) {
                    if entry.success == success_only {
                        ordered.push_back(entry);
                    }
                }
            }
        }

        ordered
    }

    /// Get the number of call log entries for a route.
    ///
    /// More efficient than loading the full call log when callers only need
    /// the current retained length.
    /// Returns the number of call log entries stored for a route.
    ///
    /// More efficient than get_call_log(route).len() as it avoids loading all entries.
    ///
    /// # Arguments
    /// * `env` - The Soroban environment.
    /// * `route` - The route name to get the log length for.
    ///
    /// # Returns
    /// The number of call log entries as `u32`.
    pub fn get_call_log_length(env: Env, route: String) -> u32 {
        env.storage()
            .instance()
            .get::<DataKey, CallLogState>(&DataKey::CallLog(route))
            .map(|log| log.entries.len())
            .unwrap_or(0)
    }

    /// Get an aggregated summary of call log stats for a route.
    ///
    /// Returns total calls, success count, failure count, and last call timestamp
    /// without loading all log entries. The summary is maintained incrementally
    /// by `post_call` whenever log retention is enabled for the route.
    ///
    /// # Arguments
    /// * `env` - The Soroban environment.
    /// * `route` - The route name to summarize.
    ///
    /// # Returns
    /// `Some(CallLogSummary)` if any calls have been logged, `None` otherwise.
    pub fn get_call_log_summary(env: Env, route: String) -> Option<CallLogSummary> {
        env.storage()
            .instance()
            .get(&DataKey::CallLogSummary(route))
    }

    /// Clear all call log entries for a route.
    ///
    /// Caller must be the admin. This allows manual clearing of the call log
    /// for a route, for example after a security incident, to start fresh
    /// without changing the retention configuration.
    ///
    /// # Arguments
    /// * `env` - The Soroban environment.
    /// * `caller` - The address initiating the call; must be the admin.
    /// * `route` - The route name to clear the call log for.
    ///
    /// # Returns
    /// `Ok(())` on success.
    ///
    /// # Errors
    /// * [`MiddlewareError::Unauthorized`] — if the caller is not the admin.
    pub fn reset_route_call_log(
        env: Env,
        caller: Address,
        route: String,
    ) -> Result<(), MiddlewareError> {
        caller.require_auth();
        router_common::require_admin_simple!(&env, &caller, &DataKey::Admin, MiddlewareError)?;
        env.storage().instance().remove(&DataKey::CallLog(route.clone()));
        env.events().publish(
            (Symbol::new(&env, "call_log_cleared"),),
            route,
        );
        Ok(())
    }

    /// Get rate limit state for a caller on a specific route.
    ///
    /// Returns the current [`RateLimitState`] for `caller` on `route`, which includes the
    /// number of calls made in the current window and when the window started.
    ///
    /// If the window has elapsed, returns a reset state with `calls_in_window = 0`
    /// and updated `window_start`.
    ///
    /// # Arguments
    /// * `env` - The Soroban environment.
    /// * `route` - The route name to look up.
    /// * `caller` - The address whose rate limit state to retrieve.
    ///
    /// # Returns
    /// `Some(`[`RateLimitState`]`)` if the caller has made at least one call on this route,
    /// `None` otherwise.
    pub fn rate_limit_state(env: Env, route: String, caller: Address) -> Option<RateLimitState> {
        let route_call_state: RouteCallState = env
            .storage()
            .instance()
            .get(&DataKey::RouteCallState(route.clone()))?;
        let state: RateLimitState = route_call_state.rate_limits.get(caller)?;

        // If route config exists, apply window expiry logic
        if let Some(config) = env
            .storage()
            .instance()
            .get::<DataKey, RouteConfig>(&DataKey::RouteConfig(route))
        {
            let now = env.ledger().timestamp();
            let window_elapsed = now >= state.window_start + config.window_seconds;

            if window_elapsed {
                Some(RateLimitState {
                    calls_in_window: 0,
                    window_start: now,
                    total_violations: state.total_violations,
                })
            } else {
                Some(state)
            }
        } else {
            // No config for this route — return raw state as-is
            Some(state)
        }
    }

    /// Get rate limit statistics for a caller on a specific route.
    ///
    /// Returns the current [`RateLimitState`] for `caller` on `route`, which includes the
    /// number of calls made in the current window, the window start time, and the total
    /// number of times the rate limit has been exceeded.
    ///
    /// If the window has elapsed, returns a reset state with `calls_in_window = 0`
    /// and updated `window_start`, but preserves the `total_violations` count.
    ///
    /// # Arguments
    /// * `env` - The Soroban environment.
    /// * `route` - The route name to look up.
    /// * `caller` - The address whose rate limit stats to retrieve.
    ///
    /// # Returns
    /// `Some(`[`RateLimitState`]`)` if the caller has made at least one call on this route,
    /// `None` otherwise.
    pub fn get_rate_limit_stats(env: Env, route: String, caller: Address) -> Option<RateLimitState> {
        Self::rate_limit_state(env, route, caller)
    }

    /// Get aggregated rate limit statistics for a route across all callers.
    ///
    /// Returns the total number of calls in the current window, the earliest window start time,
    /// and the total number of rate limit violations across all callers for the given route.
    ///
    /// # Arguments
    /// * `env` - The Soroban environment.
    /// * `route` - The route name to look up.
    ///
    /// # Returns
    /// `Some(`[`RouteRateLimitStats`]`)` if any caller has made calls on this route,
    /// `None` otherwise.
    pub fn get_route_rate_limit_stats(env: Env, route: String) -> Option<RouteRateLimitStats> {
        let route_call_state: RouteCallState = env
            .storage()
            .instance()
            .get(&DataKey::RouteCallState(route.clone()))?;

        if route_call_state.rate_limits.is_empty() {
            return None;
        }

        let mut total_calls_in_window: u32 = 0;
        let mut total_violations: u32 = 0;
        let mut earliest_window_start: u64 = u64::MAX;

        // Get route config to apply window expiry logic
        let config = env
            .storage()
            .instance()
            .get::<DataKey, RouteConfig>(&DataKey::RouteConfig(route.clone()));
        let now = env.ledger().timestamp();

        for (_caller, state) in route_call_state.rate_limits.iter() {
            let (calls, window_start) = if let Some(ref cfg) = config {
                let window_elapsed = now >= state.window_start + cfg.window_seconds;
                if window_elapsed {
                    (0, now)
                } else {
                    (state.calls_in_window, state.window_start)
                }
            } else {
                (state.calls_in_window, state.window_start)
            };

            total_calls_in_window += calls;
            total_violations += state.total_violations;
            if window_start < earliest_window_start {
                earliest_window_start = window_start;
            }
        }

        // If no valid window start was found, use current time
        let final_window_start = if earliest_window_start == u64::MAX {
            now
        } else {
            earliest_window_start
        };

        Some(RouteRateLimitStats {
            total_calls_in_window,
            window_start: final_window_start,
            total_violations,
        })
    }

    /// Reset rate limit state for a caller on a specific route.
    ///
    /// Clears the rate limit storage key for the given caller/route pair, allowing
    /// the caller to make calls again without waiting for the window to expire.
    /// Caller must be the admin.
    ///
    /// # Arguments
    /// * `env` - The Soroban environment.
    /// * `caller` - The address initiating the call; must be the admin.
    /// * `route` - The route name to reset the rate limit for.
    /// * `target_caller` - The address whose rate limit state should be reset.
    ///
    /// # Returns
    /// `Ok(())` on success.
    ///
    /// # Errors
    /// * [`MiddlewareError::Unauthorized`] — if `caller` is not the admin.
    /// * [`MiddlewareError::NotInitialized`] — if the contract has not been initialized.
    pub fn reset_rate_limit(
        env: Env,
        caller: Address,
        route: String,
        target_caller: Address,
    ) -> Result<(), MiddlewareError> {
        caller.require_auth();
        router_common::require_admin_simple!(&env, &caller, &DataKey::Admin, MiddlewareError)?;

        let mut route_call_state: RouteCallState = env
            .storage()
            .instance()
            .get(&DataKey::RouteCallState(route.clone()))
            .unwrap_or(RouteCallState {
                rate_limits: Map::new(&env),
                circuit_breaker: CircuitBreakerState {
                    failure_count: 0,
                    opened_at: 0,
                    is_open: false,
                    is_half_open: false,
                },
            });

        route_call_state.rate_limits.remove(target_caller.clone());
        env.storage()
            .instance()
            .set(&DataKey::RouteCallState(route), &route_call_state);

        Ok(())
    }

    ///
    /// Returns the [`RouteConfig`] for `route` if one has been set via
    /// `configure_route`.
    ///
    /// # Arguments
    /// * `env` - The Soroban environment.
    /// * `route` - The route name to look up.
    ///
    /// # Returns
    /// `Some(`[`RouteConfig`]`)` if a config exists for `route`, `None` otherwise.
    pub fn route_config(env: Env, route: String) -> Option<RouteConfig> {
        env.storage().instance().get(&DataKey::RouteConfig(route))
    }

    /// Returns all route names that have been configured via configure_route.
    ///
    /// # Arguments
    /// * `env` - The Soroban environment.
    ///
    /// # Returns
    /// A `Vec<String>` of unique route names passed to `configure_route`.
    pub fn get_configured_routes(env: Env) -> Vec<String> {
        env.storage()
            .instance()
            .get(&DataKey::ConfiguredRoutes)
            .unwrap_or_else(|| Vec::new(&env))
    }

    /// Get the current circuit breaker state for a route.
    ///
    /// Returns `None` if no circuit breaker state has been recorded for the route
    /// (i.e. no failures have occurred since initialization or last reset).
    ///
    /// # Arguments
    /// * `env` - The Soroban environment.
    /// * `route` - The route name to query.
    ///
    /// # Returns
    /// `Some(CircuitBreakerState)` if state exists, `None` otherwise.
    pub fn circuit_breaker_state(env: Env, route: String) -> Option<CircuitBreakerState> {
        let route_call_state: RouteCallState = env
            .storage()
            .instance()
            .get(&DataKey::RouteCallState(route))?;
        Some(route_call_state.circuit_breaker)
    }

    /// Get current admin.
    ///
    /// # Arguments
    /// * `env` - The Soroban environment.
    ///
    /// # Returns
    /// The [`Address`] of the current admin.
    ///
    /// # Panics
    /// * Panics if the contract has not been initialized.
    /// 
    /// Get the current admin address.
    ///
    /// # Errors
    /// Returns `MiddlewareError::NotInitialized` if the contract has not been initialized.
    pub fn admin(env: Env) -> Result<Address, MiddlewareError> {
        env.storage()
            .instance()
            .get(&DataKey::Admin)
            .ok_or(MiddlewareError::NotInitialized)
    }

    /// Reset circuit breaker for a route.
    ///
    /// Manually resets the circuit breaker state for a route, clearing the
    /// failure count and closing the circuit. Caller must be the admin.
    ///
    /// # Arguments
    /// * `env` - The Soroban environment.
    /// * `caller` - The address initiating the call; must be the admin.
    /// * `route` - The route name whose circuit breaker should be reset.
    ///
    /// # Returns
    /// `Ok(())` on success.
    ///
    /// # Errors
    /// * [`MiddlewareError::Unauthorized`] — if `caller` is not the admin.
    /// * [`MiddlewareError::NotInitialized`] — if the contract has not been initialized.
    pub fn reset_circuit_breaker(
        env: Env,
        caller: Address,
        route: String,
    ) -> Result<(), MiddlewareError> {
        caller.require_auth();
        router_common::require_admin_simple!(&env, &caller, &DataKey::Admin, MiddlewareError)?;

        let reset_state = CircuitBreakerState {
            failure_count: 0,
            opened_at: 0,
            is_open: false,
            is_half_open: false,
        };
        let mut route_call_state: RouteCallState = env
            .storage()
            .instance()
            .get(&DataKey::RouteCallState(route.clone()))
            .unwrap_or(RouteCallState {
                rate_limits: Map::new(&env),
                circuit_breaker: CircuitBreakerState {
                    failure_count: 0,
                    opened_at: 0,
                    is_open: false,
                    is_half_open: false,
                },
            });
        route_call_state.circuit_breaker = reset_state;
        env.storage()
            .instance()
            .set(&DataKey::RouteCallState(route), &route_call_state);
        Ok(())
    }

    /// Transfer admin to a new address.
    ///
    /// Replaces the current admin with `new_admin`. The `current` address must
    /// authenticate and must be the existing admin.
    ///
    /// # Arguments
    /// * `env` - The Soroban environment.
    /// * `current` - The current admin address; must authenticate.
    /// * `new_admin` - The address that will become the new admin.
    ///
    /// # Returns
    /// `Ok(())` on success.
    ///
    /// # Errors
    /// * [`MiddlewareError::Unauthorized`] — if `current` is not the admin.
    /// * [`MiddlewareError::NotInitialized`] — if the contract has not been initialized.
    pub fn transfer_admin(
        env: Env,
        current: Address,
        new_admin: Address,
    ) -> Result<(), MiddlewareError> {
        current.require_auth();
        router_common::require_admin_simple!(&env, &current, &DataKey::Admin, MiddlewareError)?;
        router_common::admin_transfer_complete!(&env, &current, &new_admin, &DataKey::Admin);
        Ok(())
    }

}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    extern crate std;
    use super::*;
    use soroban_sdk::{
        testutils::{Address as _, Events, Ledger},
        Env, IntoVal, String,
    };

    fn setup() -> (Env, Address, RouterMiddlewareClient<'static>) {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register_contract(None, RouterMiddleware);
        let client = RouterMiddlewareClient::new(&env, &contract_id);
        let admin = Address::generate(&env);
        client.initialize(&admin);
        (env, admin, client)
    }

    #[test]
    fn test_rate_limit_state_not_written_when_route_disabled_before_commit() {
        // Verifies that if a route is disabled after the initial enabled check
        // but before the commit phase, no stale rate-limit state is written.
        let (env, admin, client) = setup();
        let route = String::from_str(&env, "oracle/get_price");
        // Enable route with a rate limit of 5 calls per window
        client.configure_route(&admin, &route, &5, &60, &true, &0, &0, &0);

        let caller = Address::generate(&env);

        // First call succeeds and writes rate limit state (calls_in_window = 1)
        assert!(client.try_pre_call(&caller, &route).is_ok());
        let state_after_first = client.rate_limit_state(&route, &caller).unwrap();
        assert_eq!(state_after_first.calls_in_window, 1);

        // Disable the route
        client.configure_route(&admin, &route, &5, &60, &false, &0, &0, &0);

        // pre_call must be rejected — RouteDisabled
        assert_eq!(
            client.try_pre_call(&caller, &route),
            Err(Ok(MiddlewareError::RouteDisabled))
        );

        // Rate limit state must NOT have advanced — still at 1, not 2
        let state_after_rejected = client.rate_limit_state(&route, &caller).unwrap();
        assert_eq!(state_after_rejected.calls_in_window, 1);

        // Total calls counter must NOT have incremented
        assert_eq!(client.total_calls(), 1);

        // Re-enable the route — no stale state should affect the next call
        client.configure_route(&admin, &route, &5, &60, &true, &0, &0, &0);
        assert!(client.try_pre_call(&caller, &route).is_ok());
        let state_after_reenable = client.rate_limit_state(&route, &caller).unwrap();
        assert_eq!(state_after_reenable.calls_in_window, 2);
    }

    #[test]
    fn test_global_disable_does_not_write_rate_limit_state() {
        let (env, admin, client) = setup();
        let route = String::from_str(&env, "oracle/get_price");
        client.configure_route(&admin, &route, &5, &60, &true, &0, &0, &0);

        let caller = Address::generate(&env);
        // One successful call
        client.pre_call(&caller, &route);
        assert_eq!(client.total_calls(), 1);

        // Disable globally
        client.set_global_enabled(&admin, &false);

        // Rejected call must not touch rate limit state or total counter
        assert_eq!(
            client.try_pre_call(&caller, &route),
            Err(Ok(MiddlewareError::MiddlewareDisabled))
        );
        assert_eq!(client.total_calls(), 1);
        let state = client.rate_limit_state(&route, &caller).unwrap();
        assert_eq!(state.calls_in_window, 1);
    }

    #[test]
    fn test_pre_call_no_config_passes() {
        let (env, _, client) = setup();
        let caller = Address::generate(&env);
        let route = String::from_str(&env, "oracle/get_price");
        let result = client.try_pre_call(&caller, &route);
        assert!(result.is_ok());
        assert_eq!(client.total_calls(), 1);
    }

    #[test]
    fn test_rate_limit_enforced() {
        let (env, admin, client) = setup();
        let route = String::from_str(&env, "oracle/get_price");
        // max 2 calls per 60s window
        client.configure_route(&admin, &route, &2, &60, &true, &0, &0, &0);

        let caller = Address::generate(&env);
        client.pre_call(&caller, &route);
        client.pre_call(&caller, &route);
        let result = client.try_pre_call(&caller, &route);
        assert_eq!(result, Err(Ok(MiddlewareError::RateLimitExceeded)));
    }

    #[test]
    fn test_rate_limit_resets_after_window() {
        let (env, admin, client) = setup();
        let route = String::from_str(&env, "oracle/get_price");
        client.configure_route(&admin, &route, &1, &60, &true, &0, &0, &0);

        let caller = Address::generate(&env);
        client.pre_call(&caller, &route);
        // Advance past window
        env.ledger().with_mut(|l| l.timestamp += 61);
        let result = client.try_pre_call(&caller, &route);
        assert!(result.is_ok());
    }

    #[test]
    fn test_disabled_route_blocked() {
        let (env, admin, client) = setup();
        let route = String::from_str(&env, "oracle/get_price");
        client.configure_route(&admin, &route, &0, &0, &false, &0, &0, &0);
        let caller = Address::generate(&env);
        let result = client.try_pre_call(&caller, &route);
        assert_eq!(result, Err(Ok(MiddlewareError::RouteDisabled)));
    }

    #[test]
    fn test_global_disable_blocks_all() {
        let (env, admin, client) = setup();
        client.set_global_enabled(&admin, &false);
        let caller = Address::generate(&env);
        let route = String::from_str(&env, "any/route");
        let result = client.try_pre_call(&caller, &route);
        assert_eq!(result, Err(Ok(MiddlewareError::MiddlewareDisabled)));
    }

    #[test]
    fn test_unauthorized_configure_fails() {
        let (env, _admin, client) = setup();
        let attacker = Address::generate(&env);
        let route = String::from_str(&env, "oracle/get_price");
        let result = client.try_configure_route(&attacker, &route, &10, &60, &true, &0, &0, &0);
        assert_eq!(result, Err(Ok(MiddlewareError::Unauthorized)));
    }

    #[test]
    fn test_post_call_succeeds() {
        let (env, _, client) = setup();
        let caller = Address::generate(&env);
        let route = String::from_str(&env, "oracle/get_price");

        // post_call should succeed with both true and false outcomes
        client.post_call(&caller, &route, &true);
        client.post_call(&caller, &route, &false);
    }

    #[test]
    fn test_get_call_log_length_zero_before_calls() {
        let (env, admin, client) = setup();
        let route = String::from_str(&env, "oracle/get_price");
        client.configure_route(&admin, &route, &0, &0, &true, &0, &0, &3);

        assert_eq!(client.get_call_log_length(&route), 0);
    }

    #[test]
    fn test_get_call_log_length_matches_get_call_log() {
        let (env, admin, client) = setup();
        let route = String::from_str(&env, "oracle/get_price");
        let caller = Address::generate(&env);
        client.configure_route(&admin, &route, &0, &0, &true, &0, &0, &5);

        client.post_call(&caller, &route, &true);
        client.post_call(&caller, &route, &false);

        assert_eq!(client.get_call_log_length(&route), client.get_call_log(&route).len());
    }

    #[test]
    fn test_get_call_log_length_respects_retention() {
        let (env, admin, client) = setup();
        let route = String::from_str(&env, "oracle/get_price");
        let caller = Address::generate(&env);
        client.configure_route(&admin, &route, &0, &0, &true, &0, &0, &2);

        client.post_call(&caller, &route, &true);
        client.post_call(&caller, &route, &false);
        client.post_call(&caller, &route, &true);

        assert_eq!(client.get_call_log_length(&route), 2);
        assert_eq!(client.get_call_log(&route).len(), 2);
    }

    #[test]
    fn test_rate_limit_isolated_per_route() {
        let (env, admin, client) = setup();
        let route_a = String::from_str(&env, "oracle/price");
        let route_b = String::from_str(&env, "vault/deposit");
        // route_a: 10 calls per minute, route_b: 5 calls per minute
        client.configure_route(&admin, &route_a, &10, &60, &true, &0, &0, &0);
        client.configure_route(&admin, &route_b, &5, &60, &true, &0, &0, &0);

        let caller = Address::generate(&env);
        // Make 4 calls on route_a — drains route_a counter to 4
        for _ in 0..4 {
            client.pre_call(&caller, &route_a);
        }
        // First call on route_b should succeed (independent counter starts at 0)
        assert!(client.try_pre_call(&caller, &route_b).is_ok());
        // Exhaust route_b (4 more calls → total 5 on route_b)
        for _ in 0..4 {
            client.pre_call(&caller, &route_b);
        }
        // route_b is now at its limit; route_a still has headroom
        assert_eq!(
            client.try_pre_call(&caller, &route_b),
            Err(Ok(MiddlewareError::RateLimitExceeded))
        );
        assert!(client.try_pre_call(&caller, &route_a).is_ok());
    }

    #[test]
    fn test_total_calls_not_incremented_on_rejected_pre_call() {
        let (env, admin, client) = setup();
        let route = String::from_str(&env, "oracle/get_price");
        client.configure_route(&admin, &route, &1, &60, &true, &0, &0, &0);

        let caller = Address::generate(&env);
        client.pre_call(&caller, &route); // passes, total = 1
        assert_eq!(client.total_calls(), 1);

        let _ = client.try_pre_call(&caller, &route); // rejected (rate limit)
        assert_eq!(client.total_calls(), 1); // must still be 1
    }

    #[test]
    fn test_admin_getter() {
        let (env, admin, client) = setup();
        let retrieved_admin = client.admin();
        assert_eq!(retrieved_admin, admin);
    }

    #[test]
    fn test_transfer_admin() {
        let (env, admin, client) = setup();
        let new_admin = Address::generate(&env);
        client.transfer_admin(&admin, &new_admin);
        assert_eq!(client.admin(), new_admin);
    }

    #[test]
    fn test_unauthorized_transfer_admin_fails() {
        let (env, _admin, client) = setup();
        let attacker = Address::generate(&env);
        let new_admin = Address::generate(&env);
        let result = client.try_transfer_admin(&attacker, &new_admin);
        assert_eq!(result, Err(Ok(MiddlewareError::Unauthorized)));
    }

    #[test]
    fn test_circuit_breaker_blocks_calls() {
        let (env, admin, client) = setup();
        let route = String::from_str(&env, "oracle/get_price");
        // Configure route with failure_threshold = 1, no recovery window for simplicity
        client.configure_route(&admin, &route, &0, &0, &true, &1, &0, &0);

        let caller = Address::generate(&env);
        // First call succeeds
        assert!(client.try_pre_call(&caller, &route).is_ok());
        // Post call with failure to trip circuit
        client.post_call(&caller, &route, &false);
        // Now pre_call should return CircuitOpen
        let result = client.try_pre_call(&caller, &route);
        assert_eq!(result, Err(Ok(MiddlewareError::CircuitOpen)));
    }

    #[test]
    fn test_reset_circuit_breaker() {
        let (env, admin, client) = setup();
        let route = String::from_str(&env, "oracle/get_price");
        client.configure_route(&admin, &route, &0, &0, &true, &1, &0, &0);

        let caller = Address::generate(&env);
        client.pre_call(&caller, &route);
        client.post_call(&caller, &route, &false);
        // Verify circuit is open
        assert_eq!(
            client.try_pre_call(&caller, &route),
            Err(Ok(MiddlewareError::CircuitOpen))
        );

        // Reset circuit
        client.reset_circuit_breaker(&admin, &route);
        // Now pre_call should succeed
        assert!(client.try_pre_call(&caller, &route).is_ok());
    }

    #[test]
    fn test_circuit_breaker_unauthorized_reset() {
        let (env, _admin, client) = setup();
        let route = String::from_str(&env, "oracle/get_price");
        let attacker = Address::generate(&env);
        let result = client.try_reset_circuit_breaker(&attacker, &route);
        assert_eq!(result, Err(Ok(MiddlewareError::Unauthorized)));
    }

    #[test]
    fn test_transfer_admin_emits_event() {
        let env = Env::default();
        env.mock_all_auths();

        let contract_id = env.register_contract(None, RouterMiddleware);
        let client = RouterMiddlewareClient::new(&env, &contract_id);

        let old_admin = Address::generate(&env);
        let new_admin = Address::generate(&env);

        // Initialize with old_admin
        client.initialize(&old_admin);

        // Perform transfer
        client.transfer_admin(&old_admin, &new_admin);

        // Verify event was emitted
        let events = env.events().all();
        let last_event = events.last().unwrap();

        assert_eq!(last_event.0, contract_id); // contract address as publisher

        // Topic should be "admin_transferred"
        let topic: Symbol = last_event.1.get(0).unwrap().into_val(&env);
        assert_eq!(topic, Symbol::new(&env, "admin_transferred"));

        // Data should contain (old_admin, new_admin)
        let (emitted_old, emitted_new): (Address, Address) = last_event.2.into_val(&env);
        assert_eq!(emitted_old, old_admin);
        assert_eq!(emitted_new, new_admin);
    }

    #[test]
    fn test_success_resets_failure_count() {
        let (env, admin, client) = setup();
        let route = String::from_str(&env, "oracle/get_price");
        // threshold=3, so 2 failures then a success then 2 more should NOT trip
        client.configure_route(&admin, &route, &0, &0, &true, &3, &0, &0);
        let caller = Address::generate(&env);

        client.post_call(&caller, &route, &false); // failure_count = 1
        client.post_call(&caller, &route, &false); // failure_count = 2
        client.post_call(&caller, &route, &true); // success → reset to 0
        client.post_call(&caller, &route, &false); // failure_count = 1
        client.post_call(&caller, &route, &false); // failure_count = 2

        // Circuit should still be closed (threshold=3, count=2)
        let result = client.try_pre_call(&caller, &route);
        assert!(result.is_ok());
    }

    #[test]
    fn test_open_circuit_not_reset_by_success() {
        let (env, admin, client) = setup();
        let route = String::from_str(&env, "oracle/get_price");
        client.configure_route(&admin, &route, &0, &0, &true, &1, &0, &0);
        let caller = Address::generate(&env);

        client.post_call(&caller, &route, &false); // trips circuit (threshold=1)
        client.post_call(&caller, &route, &true); // success — must NOT reset is_open

        // Circuit must still be open
        assert_eq!(
            client.try_pre_call(&caller, &route),
            Err(Ok(MiddlewareError::CircuitOpen))
        );
    }

    // ── Issue #150: get_configured_routes ────────────────────────────────────

    #[test]
    fn test_get_configured_routes_empty() {
        let (_env, _admin, client) = setup();
        let routes = client.get_configured_routes();
        assert!(routes.is_empty());
    }

    #[test]
    fn test_get_configured_routes_multiple() {
        let (env, admin, client) = setup();
        let route_a = String::from_str(&env, "oracle/price");
        let route_b = String::from_str(&env, "vault/deposit");
        client.configure_route(&admin, &route_a, &0, &0, &true, &0, &0, &0);
        client.configure_route(&admin, &route_b, &0, &0, &true, &0, &0, &0);
        let routes = client.get_configured_routes();
        assert_eq!(routes.len(), 2);
        assert!(routes.contains(&route_a));
        assert!(routes.contains(&route_b));
    }

    #[test]
    fn test_get_configured_routes_no_duplicates() {
        let (env, admin, client) = setup();
        let route = String::from_str(&env, "oracle/price");
        client.configure_route(&admin, &route, &0, &0, &true, &0, &0, &0);
        client.configure_route(&admin, &route, &5, &60, &true, &0, &0, &0);
        let routes = client.get_configured_routes();
        assert_eq!(routes.len(), 1);
    }

    // ── Issue #155: circuit_breaker_state getter ──────────────────────────────

    #[test]
    fn test_circuit_breaker_state_none_before_failures() {
        let (env, admin, client) = setup();
        let route = String::from_str(&env, "oracle/get_price");
        client.configure_route(&admin, &route, &0, &0, &true, &3, &0, &0);
        assert_eq!(client.circuit_breaker_state(&route), None);
    }

    #[test]
    fn test_circuit_breaker_state_reflects_failures() {
        let (env, admin, client) = setup();
        let route = String::from_str(&env, "oracle/get_price");
        client.configure_route(&admin, &route, &0, &0, &true, &3, &0, &0);
        let caller = Address::generate(&env);
        client.post_call(&caller, &route, &false);
        let state = client.circuit_breaker_state(&route).unwrap();
        assert_eq!(state.failure_count, 1);
        assert!(!state.is_open);
    }

    #[test]
    fn test_circuit_breaker_state_open_after_threshold() {
        let (env, admin, client) = setup();
        let route = String::from_str(&env, "oracle/get_price");
        client.configure_route(&admin, &route, &0, &0, &true, &2, &0, &0);
        let caller = Address::generate(&env);
        client.post_call(&caller, &route, &false);
        client.post_call(&caller, &route, &false);
        let state = client.circuit_breaker_state(&route).unwrap();
        assert!(state.is_open);
        assert!(state.opened_at > 0);
    }

    #[test]
    fn test_circuit_breaker_state_clears_after_reset() {
        let (env, admin, client) = setup();
        let route = String::from_str(&env, "oracle/get_price");
        client.configure_route(&admin, &route, &0, &0, &true, &1, &0, &0);
        let caller = Address::generate(&env);
        client.post_call(&caller, &route, &false);
        assert!(client.circuit_breaker_state(&route).unwrap().is_open);
        client.reset_circuit_breaker(&admin, &route);
        let state = client.circuit_breaker_state(&route).unwrap();
        assert!(!state.is_open);
    }

    // ── Issue: log retention off-by-one ──────────────────────────────────────

    #[test]
    fn test_call_log_never_exceeds_retention() {
        let (env, admin, client) = setup();
        let route = String::from_str(&env, "oracle/get_price");
        client.configure_route(&admin, &route, &0, &0, &true, &0, &0, &3);

        let caller = Address::generate(&env);
        for _ in 0..10 {
            client.pre_call(&caller, &route);
            client.post_call(&caller, &route, &true);
        }

        assert_eq!(client.get_call_log(&route).len(), 3);
    }

    #[test]
    fn test_call_log_retains_most_recent() {
        let (env, admin, client) = setup();
        let route = String::from_str(&env, "oracle/get_price");
        client.configure_route(&admin, &route, &0, &0, &true, &0, &0, &3);

        let caller = Address::generate(&env);
        // Make 5 calls with distinct timestamps so we can identify order
        for i in 0..5u64 {
            env.ledger().set_timestamp(1000 + i);
            client.pre_call(&caller, &route);
            client.post_call(&caller, &route, &true);
        }

        let log = client.get_call_log(&route);
        assert_eq!(log.len(), 3);
        // Oldest retained entry should be call #2 (timestamp 1002)
        assert_eq!(log.get(0).unwrap().timestamp, 1002);
        // Newest entry should be call #4 (timestamp 1004)
        assert_eq!(log.get(2).unwrap().timestamp, 1004);
    }

    // ── Issue #154: rate_limit_state window expiry ────────────────────────────

    #[test]
    fn test_rate_limit_state_resets_after_window() {
        let (env, admin, client) = setup();
        let route = String::from_str(&env, "oracle/get_price");
        // Configure route with 60 second window and max 5 calls
        client.configure_route(&admin, &route, &5, &60, &true, &0, &0, &0);

        let caller = Address::generate(&env);

        // Make 3 calls
        for _ in 0..3 {
            client.pre_call(&caller, &route);
        }

        // Check state within window — should show 3 calls
        let state_within_window = client.rate_limit_state(&route, &caller).unwrap();
        assert_eq!(state_within_window.calls_in_window, 3);

        // Advance time past the window
        env.ledger().set_timestamp(env.ledger().timestamp() + 61);

        // Check state after window expires — should reset to 0
        let state_after_window = client.rate_limit_state(&route, &caller).unwrap();
        assert_eq!(state_after_window.calls_in_window, 0);
        assert_eq!(
            state_after_window.window_start,
            env.ledger().timestamp() - 1
        );
    }

    #[test]
    fn test_rate_limit_state_within_window_accurate() {
        let (env, admin, client) = setup();
        let route = String::from_str(&env, "oracle/get_price");
        // Configure route with 60 second window
        client.configure_route(&admin, &route, &5, &60, &true, &0, &0, &0);

        let caller = Address::generate(&env);

        // Make 2 calls
        client.pre_call(&caller, &route);
        client.pre_call(&caller, &route);

        // Check state — should show 2 calls
        let state = client.rate_limit_state(&route, &caller).unwrap();
        assert_eq!(state.calls_in_window, 2);

        // Advance time but stay within window (30 seconds, window is 60)
        env.ledger().set_timestamp(env.ledger().timestamp() + 30);

        // State should still show 2 calls, not reset
        let state_still_in_window = client.rate_limit_state(&route, &caller).unwrap();
        assert_eq!(state_still_in_window.calls_in_window, 2);
        assert_eq!(
            state_still_in_window.window_start, state.window_start,
            "window_start should not change within window"
        );
    }

    #[test]
    fn test_circuit_breaker_auto_recovers_after_window() {
        let (env, admin, client) = setup();
        let route = String::from_str(&env, "oracle/get_price");
        // failure_threshold=1, recovery_window=60s
        client.configure_route(&admin, &route, &0, &0, &true, &1, &60, &0);

        let caller = Address::generate(&env);

        // Trip the circuit
        client.post_call(&caller, &route, &false);
        assert_eq!(
            client.try_pre_call(&caller, &route),
            Err(Ok(MiddlewareError::CircuitOpen))
        );

        // Advance time past recovery window
        env.ledger().with_mut(|l| l.timestamp += 61);

        // Call should now succeed (auto-recovery)
        assert!(client.try_pre_call(&caller, &route).is_ok());
    }

    #[test]
    fn test_circuit_not_recovered_before_window_elapses() {
        let (env, admin, client) = setup();
        let route = String::from_str(&env, "oracle/get_price");
        client.configure_route(&admin, &route, &0, &0, &true, &1, &60, &0);

        let caller = Address::generate(&env);
        client.post_call(&caller, &route, &false);

        // Only 30 seconds — not enough
        env.ledger().with_mut(|l| l.timestamp += 30);
        assert_eq!(
            client.try_pre_call(&caller, &route),
            Err(Ok(MiddlewareError::CircuitOpen))
        );
    }

    #[test]
    fn test_circuit_breaker_state_reset_after_recovery() {
        let (env, admin, client) = setup();
        let route = String::from_str(&env, "oracle/get_price");
        // failure_threshold=1, recovery_window=60s
        client.configure_route(&admin, &route, &0, &0, &true, &1, &60, &0);

        let caller = Address::generate(&env);

        // Trip the circuit
        client.post_call(&caller, &route, &false);

        // Verify circuit is open in storage
        let state_when_open = client.circuit_breaker_state(&route).unwrap();
        assert!(state_when_open.is_open);
        assert_eq!(state_when_open.failure_count, 1);

        // Call should be blocked
        assert_eq!(
            client.try_pre_call(&caller, &route),
            Err(Ok(MiddlewareError::CircuitOpen))
        );

        // Advance time past recovery window
        env.ledger().with_mut(|l| l.timestamp += 61);

        // Call should now succeed (auto-recovery)
        assert!(client.try_pre_call(&caller, &route).is_ok());

        // Verify circuit breaker state is reset in storage
        let state_after_recovery = client.circuit_breaker_state(&route).unwrap();
        assert!(!state_after_recovery.is_open);
        assert_eq!(state_after_recovery.failure_count, 0);
        assert_eq!(state_after_recovery.opened_at, 0);
    }

    // ── Issue #455: rate limit window boundary conditions ─────────────────────

    #[test]
    fn test_rate_limit_call_at_exact_window_boundary_resets() {
        // A call at exactly window_start + window_seconds should start a new window.
        let (env, admin, client) = setup();
        let route = String::from_str(&env, "oracle/price");
        // max 1 call per 60s window
        client.configure_route(&admin, &route, &1, &60, &true, &0, &0, &0);

        let caller = Address::generate(&env);
        let t0 = env.ledger().timestamp();

        // Exhaust the window
        client.pre_call(&caller, &route);
        assert_eq!(
            client.try_pre_call(&caller, &route),
            Err(Ok(MiddlewareError::RateLimitExceeded))
        );

        // Advance to exactly window_start + window_seconds
        env.ledger().with_mut(|l| l.timestamp = t0 + 60);

        // window_elapsed = now >= window_start + window_seconds → true → new window
        assert!(client.try_pre_call(&caller, &route).is_ok());
    }

    #[test]
    fn test_rate_limit_window_jump_multiple_windows() {
        // Ledger timestamp jumps several windows at once; counter must reset.
        let (env, admin, client) = setup();
        let route = String::from_str(&env, "oracle/price");
        client.configure_route(&admin, &route, &1, &60, &true, &0, &0, &0);

        let caller = Address::generate(&env);
        let t0 = env.ledger().timestamp();

        // Exhaust window
        client.pre_call(&caller, &route);

        // Jump 5 full windows ahead
        env.ledger().with_mut(|l| l.timestamp = t0 + 300);

        // Should succeed — counter reset regardless of how many windows elapsed
        assert!(client.try_pre_call(&caller, &route).is_ok());
        let state = client.rate_limit_state(&route, &caller).unwrap();
        assert_eq!(state.calls_in_window, 1);
        assert_eq!(state.window_start, t0 + 300);
    }

    #[test]
    fn test_configure_route_window_zero_max_zero_is_unlimited() {
        // window_seconds=0 and max_calls=0 means unlimited (no rate limiting).
        let (env, admin, client) = setup();
        let route = String::from_str(&env, "oracle/price");
        client.configure_route(&admin, &route, &0, &0, &true, &0, &0, &0);

        let caller = Address::generate(&env);
        // Many calls should all succeed
        for _ in 0..20 {
            assert!(client.try_pre_call(&caller, &route).is_ok());
        }
    }

    // ── Issue #311: set_global_enabled emits event ────────────────────────────

    #[test]
    fn test_set_global_enabled_emits_event() {
        let (env, admin, client) = setup();
        client.set_global_enabled(&admin, &false);
        let events = env.events().all();
        let last = events.last().unwrap();
        let topic: Symbol = last.1.get(0).unwrap().into_val(&env);
        assert_eq!(topic, Symbol::new(&env, "middleware_enabled"));
        let emitted: bool = last.2.into_val(&env);
        assert!(!emitted);
    }

    // ── Issue #491: get_call_log_filtered ────────────────────────────────────

    #[test]
    fn test_get_call_log_filtered_empty_when_no_calls() {
        let (env, admin, client) = setup();
        let route = String::from_str(&env, "oracle/get_price");
        client.configure_route(&admin, &route, &0, &0, &true, &0, &0, &10);

        assert!(client.get_call_log_filtered(&route, &true).is_empty());
        assert!(client.get_call_log_filtered(&route, &false).is_empty());
    }

    #[test]
    fn test_get_call_log_filtered_success_only() {
        let (env, admin, client) = setup();
        let route = String::from_str(&env, "oracle/get_price");
        let caller = Address::generate(&env);
        client.configure_route(&admin, &route, &0, &0, &true, &0, &0, &10);

        client.post_call(&caller, &route, &true);
        client.post_call(&caller, &route, &false);
        client.post_call(&caller, &route, &true);

        let filtered = client.get_call_log_filtered(&route, &true);
        assert_eq!(filtered.len(), 2);
        assert!(filtered.get(0).unwrap().success);
        assert!(filtered.get(1).unwrap().success);
    }

    #[test]
    fn test_get_call_log_filtered_failure_only() {
        let (env, admin, client) = setup();
        let route = String::from_str(&env, "oracle/get_price");
        let caller = Address::generate(&env);
        client.configure_route(&admin, &route, &0, &0, &true, &0, &0, &10);

        client.post_call(&caller, &route, &true);
        client.post_call(&caller, &route, &false);
        client.post_call(&caller, &route, &false);

        let filtered = client.get_call_log_filtered(&route, &false);
        assert_eq!(filtered.len(), 2);
        assert!(!filtered.get(0).unwrap().success);
        assert!(!filtered.get(1).unwrap().success);
    }

    #[test]
    fn test_get_call_log_filtered_all_success_no_failures() {
        let (env, admin, client) = setup();
        let route = String::from_str(&env, "oracle/get_price");
        let caller = Address::generate(&env);
        client.configure_route(&admin, &route, &0, &0, &true, &0, &0, &5);

        client.post_call(&caller, &route, &true);
        client.post_call(&caller, &route, &true);

        assert!(client.get_call_log_filtered(&route, &false).is_empty());
        assert_eq!(client.get_call_log_filtered(&route, &true).len(), 2);
    }

    #[test]
    fn test_get_call_log_filtered_with_ring_buffer_wraparound() {
        let (env, admin, client) = setup();
        let route = String::from_str(&env, "oracle/get_price");
        let caller = Address::generate(&env);
        // retention=3, make 5 calls so ring buffer wraps
        client.configure_route(&admin, &route, &0, &0, &true, &0, &0, &3);

        client.post_call(&caller, &route, &true);  // evicted
        client.post_call(&caller, &route, &false); // evicted
        client.post_call(&caller, &route, &true);  // retained
        client.post_call(&caller, &route, &false); // retained
        client.post_call(&caller, &route, &true);  // retained

        // 3 retained: success, failure, success
        let success = client.get_call_log_filtered(&route, &true);
        let failure = client.get_call_log_filtered(&route, &false);
        assert_eq!(success.len(), 2);
        assert_eq!(failure.len(), 1);
    }

    // ── Issue #449: get_call_log_summary ─────────────────────────────────────

    #[test]
    fn test_get_call_log_summary_none_before_calls() {
        let (env, admin, client) = setup();
        let route = String::from_str(&env, "oracle/get_price");
        client.configure_route(&admin, &route, &0, &0, &true, &0, &0, &5);
        assert_eq!(client.get_call_log_summary(&route), None);
    }

    #[test]
    fn test_get_call_log_summary_counts_correctly() {
        let (env, admin, client) = setup();
        let route = String::from_str(&env, "oracle/get_price");
        let caller = Address::generate(&env);
        client.configure_route(&admin, &route, &0, &0, &true, &0, &0, &10);

        client.post_call(&caller, &route, &true);
        client.post_call(&caller, &route, &true);
        client.post_call(&caller, &route, &false);

        let summary = client.get_call_log_summary(&route).unwrap();
        assert_eq!(summary.total_calls, 3);
        assert_eq!(summary.success_count, 2);
        assert_eq!(summary.failure_count, 1);
    }

    #[test]
    fn test_get_call_log_summary_last_call_timestamp() {
        let (env, admin, client) = setup();
        let route = String::from_str(&env, "oracle/get_price");
        let caller = Address::generate(&env);
        client.configure_route(&admin, &route, &0, &0, &true, &0, &0, &10);

        env.ledger().set_timestamp(1000);
        client.post_call(&caller, &route, &true);
        env.ledger().set_timestamp(2000);
        client.post_call(&caller, &route, &false);

        let summary = client.get_call_log_summary(&route).unwrap();
        assert_eq!(summary.last_call_timestamp, 2000);
    }

    #[test]
    fn test_get_call_log_summary_not_affected_by_retention_limit() {
        // Summary counts all calls ever, even when the log is capped by retention
        let (env, admin, client) = setup();
        let route = String::from_str(&env, "oracle/get_price");
        let caller = Address::generate(&env);
        client.configure_route(&admin, &route, &0, &0, &true, &0, &0, &2); // retain only 2

        for _ in 0..5 {
            client.post_call(&caller, &route, &true);
        }

        let summary = client.get_call_log_summary(&route).unwrap();
        assert_eq!(summary.total_calls, 5); // all 5 counted
        assert_eq!(client.get_call_log(&route).len(), 2); // only 2 retained
    }

    // ── Issue #507: circuit breaker auto-recovery after window ───────────────

    #[test]
    fn test_circuit_opens_after_failure_threshold() {
        let (env, admin, client) = setup();
        let route = String::from_str(&env, "oracle/get_price");
        let caller = Address::generate(&env);
        
        // Configure with failure_threshold=3
        client.configure_route(&admin, &route, &0, &0, &true, &3, &60, &0);

        // Trigger 3 failures to reach threshold
        client.post_call(&caller, &route, &false);
        client.post_call(&caller, &route, &false);
        client.post_call(&caller, &route, &false);

        // Circuit should now be open
        let result = client.try_pre_call(&caller, &route);
        assert_eq!(result, Err(Ok(MiddlewareError::CircuitOpen)));
    }

    #[test]
    fn test_pre_call_blocked_while_circuit_open() {
        let (env, admin, client) = setup();
        let route = String::from_str(&env, "oracle/get_price");
        let caller = Address::generate(&env);
        
        // Configure with failure_threshold=1, recovery_window=60s
        client.configure_route(&admin, &route, &0, &0, &true, &1, &60, &0);

        // Trip the circuit
        client.post_call(&caller, &route, &false);

        // Verify circuit is open
        assert_eq!(
            client.try_pre_call(&caller, &route),
            Err(Ok(MiddlewareError::CircuitOpen))
        );

        // Multiple attempts should all be blocked
        assert_eq!(
            client.try_pre_call(&caller, &route),
            Err(Ok(MiddlewareError::CircuitOpen))
        );
        assert_eq!(
            client.try_pre_call(&caller, &route),
            Err(Ok(MiddlewareError::CircuitOpen))
        );
    }

    #[test]
    fn test_pre_call_succeeds_after_recovery_window() {
        let (env, admin, client) = setup();
        let route = String::from_str(&env, "oracle/get_price");
        let caller = Address::generate(&env);
        
        // Configure with failure_threshold=2, recovery_window=100s
        client.configure_route(&admin, &route, &0, &0, &true, &2, &100, &0);

        // Trip the circuit
        client.post_call(&caller, &route, &false);
        client.post_call(&caller, &route, &false);

        // Verify circuit is open
        assert_eq!(
            client.try_pre_call(&caller, &route),
            Err(Ok(MiddlewareError::CircuitOpen))
        );

        // Advance ledger timestamp past recovery window
        env.ledger().with_mut(|l| l.timestamp += 101);

        // pre_call should now succeed (auto-recovery)
        let result = client.try_pre_call(&caller, &route);
        assert!(result.is_ok(), "pre_call should succeed after recovery window");
    }

    #[test]
    fn test_success_after_recovery_resets_failure_count() {
        let (env, admin, client) = setup();
        let route = String::from_str(&env, "oracle/get_price");
        let caller = Address::generate(&env);
        
        // Configure with failure_threshold=2, recovery_window=60s
        client.configure_route(&admin, &route, &0, &0, &true, &2, &60, &0);

        // Trip the circuit
        client.post_call(&caller, &route, &false);
        client.post_call(&caller, &route, &false);

        // Verify circuit is open
        let state_before_recovery = client.circuit_breaker_state(&route).unwrap();
        assert!(state_before_recovery.is_open);
        assert_eq!(state_before_recovery.failure_count, 2);

        // Advance past recovery window
        env.ledger().with_mut(|l| l.timestamp += 61);

        // Make a successful call (triggers auto-recovery)
        assert!(client.try_pre_call(&caller, &route).is_ok());

        // Verify failure_count is reset to zero
        let state_after_recovery = client.circuit_breaker_state(&route).unwrap();
        assert!(!state_after_recovery.is_open);
        assert_eq!(state_after_recovery.failure_count, 0);
        assert_eq!(state_after_recovery.opened_at, 0);
    }

    #[test]
    fn test_circuit_breaker_state_updated_after_recovery() {
        let (env, admin, client) = setup();
        let route = String::from_str(&env, "oracle/get_price");
        let caller = Address::generate(&env);
        
        // Configure with failure_threshold=1, recovery_window=50s
        client.configure_route(&admin, &route, &0, &0, &true, &1, &50, &0);

        // Trip the circuit
        client.post_call(&caller, &route, &false);

        // Verify initial state
        let state_open = client.circuit_breaker_state(&route).unwrap();
        assert!(state_open.is_open);
        assert_eq!(state_open.failure_count, 1);
        assert!(state_open.opened_at > 0);

        // Advance past recovery window
        env.ledger().with_mut(|l| l.timestamp += 51);

        // Trigger auto-recovery by calling pre_call
        assert!(client.try_pre_call(&caller, &route).is_ok());

        // Verify state is fully reset
        let state_recovered = client.circuit_breaker_state(&route).unwrap();
        assert!(!state_recovered.is_open, "is_open should be false");
        assert_eq!(state_recovered.failure_count, 0, "failure_count should be 0");
        assert_eq!(state_recovered.opened_at, 0, "opened_at should be 0");
    }

    #[test]
    fn test_circuit_not_recovered_before_window_expires() {
        let (env, admin, client) = setup();
        let route = String::from_str(&env, "oracle/get_price");
        let caller = Address::generate(&env);
        
        // Configure with failure_threshold=1, recovery_window=100s
        client.configure_route(&admin, &route, &0, &0, &true, &1, &100, &0);

        // Trip the circuit
        client.post_call(&caller, &route, &false);

        // Advance time but not enough (only 50 seconds, need 100)
        env.ledger().with_mut(|l| l.timestamp += 50);

        // Circuit should still be open
        assert_eq!(
            client.try_pre_call(&caller, &route),
            Err(Ok(MiddlewareError::CircuitOpen))
        );

        // Advance to exactly the recovery time (not past it)
        env.ledger().with_mut(|l| l.timestamp += 50);

        // Should now succeed (at exactly recovery_window_seconds)
        assert!(client.try_pre_call(&caller, &route).is_ok());
    }
}

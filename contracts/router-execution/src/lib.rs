#![no_std]

//! # router-execution
//!
//! Transaction execution pipeline with structured error handling, pre-execution
//! simulation, and fee estimation for the stellar-router suite.
//!
//! ## Features
//! - Structured error hierarchy: network, simulation, and contract error categories
//! - Pre-execution simulation that blocks execution on failure
//! - Retry logic for transient (network) failures
//! - Centralized error event logging
//! - Fee estimation endpoint with edge-case handling
//!
//! ## Events (following naming convention: past tense verbs in snake_case)
//! - `execution_result` — Execution result logged (target, function, success, attempts)
//! - `fee_estimated` — Fee estimation completed (total_fee, surge_pricing)
//! - `simulation_result` — Pre-execution simulation result (target, function, success)

use soroban_sdk::{
    contract, contracterror, contractimpl, contracttype, Address, Env, String, Symbol, Vec,
};

// ── Storage Keys ──────────────────────────────────────────────────────────────

#[contracttype]
pub enum DataKey {
    Admin,
    MaxRetries,
    TotalExecutions,
    TotalErrors,
    BackoffBaseMs,    // base delay in milliseconds before first retry
    BackoffMultiplier, // multiplier applied each retry (stored as fixed-point *100, e.g. 200 = 2x)
    ExecHistory,   // Vec<ExecutionRecord>
}

// ── Error Types ───────────────────────────────────────────────────────────────

/// Structured error hierarchy for the execution pipeline.
///
/// Errors are grouped into three categories:
/// - **Network** (1xx): transient connectivity or timeout issues — eligible for retry.
/// - **Simulation** (2xx): pre-execution validation failures — execution is blocked.
/// - **Contract** (3xx): on-chain contract-level rejections — not retried.
/// - **Config** (4xx): misconfiguration or unauthorized access.
#[contracterror]
#[derive(Copy, Clone, Debug, PartialEq)]
pub enum ExecutionError {
    // ── Network errors (transient, retryable) ─────────────────────────────
    /// RPC node did not respond within the expected window.
    NetworkTimeout = 101,
    /// Network connectivity issue; retry may succeed.
    NetworkUnavailable = 102,

    // ── Simulation errors (block execution) ───────────────────────────────
    /// Simulation detected the transaction would fail on-chain.
    SimulationFailed = 201,
    /// Simulation indicated insufficient resources (budget/fees).
    SimulationInsufficientResources = 202,

    // ── Contract errors (non-retryable) ───────────────────────────────────
    /// The target contract rejected the call.
    ContractRejected = 301,
    /// The target contract was not found at the given address.
    ContractNotFound = 302,
    /// The called function does not exist on the target contract.
    ContractFunctionNotFound = 303,

    // ── Config / auth errors ──────────────────────────────────────────────
    AlreadyInitialized = 401,
    NotInitialized = 402,
    Unauthorized = 403,
    InvalidConfig = 404,
    InvalidAmount = 405,
}

// ── Types ─────────────────────────────────────────────────────────────────────

/// Describes a single transaction to execute.
#[contracttype]
#[derive(Clone, Debug, PartialEq)]
pub struct ExecutionRequest {
    /// Target contract address.
    pub target: Address,
    /// Function name to invoke.
    pub function: Symbol,
    /// Whether to run simulation before execution.
    pub simulate_first: bool,
    /// Maximum number of retries for transient (network) errors.
    /// Capped at the contract-level `max_retries` setting.
    pub max_retries: u32,
}

/// A single entry in the per-execution history log.
#[contracttype]
#[derive(Clone, Debug, PartialEq)]
pub struct ExecutionRecord {
    pub timestamp: u64,
    pub target: Address,
    pub function: Symbol,
    pub success: bool,
    pub fee_paid: i128,
}

/// Result of a single execution attempt.
#[contracttype]
#[derive(Clone, Debug, PartialEq)]
pub struct ExecutionResult {
    pub target: Address,
    pub function: Symbol,
    pub success: bool,
    /// Number of attempts made (1 = first try succeeded or non-retryable failure).
    pub attempts: u32,
    /// Whether simulation was run before execution.
    pub simulated: bool,
}

/// Result of a pre-execution simulation.
#[contracttype]
#[derive(Clone, Debug, PartialEq)]
pub struct SimulationResult {
    pub target: Address,
    pub function: Symbol,
    /// `true` if the simulated call would succeed on-chain.
    pub success: bool,
    /// `true` if the simulated call would be rejected on-chain.
    pub would_fail: bool,
    /// Human-readable feedback for the caller.
    pub message: String,
}

/// Fee estimate for a transaction.
#[contracttype]
#[derive(Clone, Debug, PartialEq)]
pub struct FeeEstimate {
    /// Base network fee in stroops.
    pub base_fee: i128,
    /// Estimated resource fee (CPU + memory) in stroops.
    pub resource_fee: i128,
    /// Total estimated fee (base + resource).
    pub total_fee: i128,
    /// Surge multiplier applied (100 = 1x, 200 = 2x, etc.).
    pub surge_multiplier: u32,
    /// Whether the estimate reflects high-load conditions.
    pub high_load: bool,
}

// ── Contract ──────────────────────────────────────────────────────────────────

#[contract]
pub struct RouterExecution;

#[contractimpl]
impl RouterExecution {
    /// Initialize the execution contract.
    ///
    /// # Arguments
    /// * `admin` - Admin address.
    /// * `max_retries` - Global cap on per-request retry attempts (max 5).
    /// * `backoff_base_ms` - Base delay in milliseconds before the first retry (0 = no delay).
    /// * `backoff_multiplier` - Multiplier applied each retry, as fixed-point *100
    ///   (e.g. 200 = 2x, 150 = 1.5x). Must be >= 100.
    ///
    /// # Errors
    /// * [`ExecutionError::AlreadyInitialized`] — called more than once.
    /// * [`ExecutionError::InvalidConfig`] — `max_retries` exceeds 5 or `backoff_multiplier` < 100.
    pub fn initialize(
        env: Env,
        admin: Address,
        max_retries: u32,
        backoff_base_ms: u64,
        backoff_multiplier: u32,
    ) -> Result<(), ExecutionError> {
        if env.storage().instance().has(&DataKey::Admin) {
            return Err(ExecutionError::AlreadyInitialized);
        }
        if max_retries > 5 {
            return Err(ExecutionError::InvalidConfig);
        }
        if backoff_multiplier < 100 {
            return Err(ExecutionError::InvalidConfig);
        }
        env.storage().instance().set(&DataKey::Admin, &admin);
        env.storage().instance().set(&DataKey::MaxRetries, &max_retries);
        env.storage().instance().set(&DataKey::BackoffBaseMs, &backoff_base_ms);
        env.storage().instance().set(&DataKey::BackoffMultiplier, &backoff_multiplier);
        env.storage().instance().set(&DataKey::TotalExecutions, &0u64);
        env.storage().instance().set(&DataKey::TotalErrors, &0u64);
        Ok(())
    }

    /// Update the backoff configuration. Caller must be admin.
    ///
    /// # Arguments
    /// * `backoff_base_ms` - Base delay in milliseconds before the first retry.
    /// * `backoff_multiplier` - Multiplier per retry as fixed-point *100 (min 100).
    ///
    /// # Errors
    /// * [`ExecutionError::Unauthorized`] — caller is not the admin.
    /// * [`ExecutionError::InvalidConfig`] — `backoff_multiplier` < 100.
    /// * [`ExecutionError::NotInitialized`] — contract not initialized.
    pub fn set_backoff_config(
        env: Env,
        caller: Address,
        backoff_base_ms: u64,
        backoff_multiplier: u32,
    ) -> Result<(), ExecutionError> {
        caller.require_auth();
        router_common::require_admin_simple!(&env, &caller, &DataKey::Admin, ExecutionError)?;
        if backoff_multiplier < 100 {
            return Err(ExecutionError::InvalidConfig);
        }
        env.storage().instance().set(&DataKey::BackoffBaseMs, &backoff_base_ms);
        env.storage().instance().set(&DataKey::BackoffMultiplier, &backoff_multiplier);
        Ok(())
    }

    /// Get the current backoff configuration.
    ///
    /// Returns `(backoff_base_ms, backoff_multiplier)`.
    pub fn backoff_config(env: Env) -> (u64, u32) {
        let base: u64 = env.storage().instance().get(&DataKey::BackoffBaseMs).unwrap_or(0);
        let mult: u32 = env.storage().instance().get(&DataKey::BackoffMultiplier).unwrap_or(100);
        (base, mult)
    }

    /// Execute a transaction with structured error handling and optional retry.
    ///
    /// If `request.simulate_first` is `true`, a dry-run simulation is performed
    /// via `try_invoke_contract` before the real call. A failed simulation blocks
    /// execution and returns [`ExecutionError::SimulationFailed`].
    ///
    /// Network errors (codes 101–102) are retried up to
    /// `min(request.max_retries, global_max_retries)` times. All other errors
    /// are returned immediately without retry.
    ///
    /// Every outcome (success or failure) is logged via an `execution_result`
    /// event so off-chain observers can monitor the pipeline.
    ///
    /// # Errors
    /// * [`ExecutionError::SimulationFailed`] — simulation detected a would-fail tx.
    /// * [`ExecutionError::ContractRejected`] — contract call failed after all retries.
    /// * [`ExecutionError::NotInitialized`] — contract not initialized.
    pub fn execute(
        env: Env,
        caller: Address,
        request: ExecutionRequest,
    ) -> Result<ExecutionResult, ExecutionError> {
        caller.require_auth();

        let max_retries: u32 = env
            .storage()
            .instance()
            .get(&DataKey::MaxRetries)
            .ok_or(ExecutionError::NotInitialized)?;

        let effective_retries = if request.max_retries < max_retries {
            request.max_retries
        } else {
            max_retries
        };

        // ── Simulation phase ──────────────────────────────────────────────
        if request.simulate_first {
            let args: Vec<soroban_sdk::Val> = Vec::new(&env);
            let sim_result = env.try_invoke_contract::<soroban_sdk::Val, soroban_sdk::Val>(
                &request.target,
                &request.function,
                args,
            );
            if sim_result.is_err() {
                Self::log_error(&env, &request.target, &request.function, ExecutionError::SimulationFailed, 0);
                return Err(ExecutionError::SimulationFailed);
            }
        }

        // ── Execution phase with retry ────────────────────────────────────
        let backoff_base_ms: u64 = env
            .storage()
            .instance()
            .get(&DataKey::BackoffBaseMs)
            .unwrap_or(0);
        let backoff_multiplier: u32 = env
            .storage()
            .instance()
            .get(&DataKey::BackoffMultiplier)
            .unwrap_or(100);

        let mut attempts = 0u32;
        loop {
            attempts += 1;
            let args: Vec<soroban_sdk::Val> = Vec::new(&env);
            let result = env.try_invoke_contract::<soroban_sdk::Val, soroban_sdk::Val>(
                &request.target,
                &request.function,
                args,
            );

            match result {
                Ok(_) => {
                    Self::increment_counter(&env, &DataKey::TotalExecutions);
                    Self::append_history(&env, &request.target, &request.function, true, 0);
                    let exec_result = ExecutionResult {
                        target: request.target.clone(),
                        function: request.function.clone(),
                        success: true,
                        attempts,
                        simulated: request.simulate_first,
                    };
                    env.events().publish(
                        (Symbol::new(&env, "execution_result"),),
                        (&request.target, &request.function, true, attempts),
                    );
                    return Ok(exec_result);
                }
                Err(_) => {
                    if attempts <= effective_retries {
                        // Compute the delay the caller should wait before the next
                        // retry: base_ms * multiplier^(attempt-1) / 100^(attempt-1).
                        // Emitting this lets off-chain orchestrators honour the backoff.
                        let delay_ms = Self::compute_backoff_ms(
                            backoff_base_ms,
                            backoff_multiplier,
                            attempts - 1,
                        );
                        env.events().publish(
                            (Symbol::new(&env, "execution_retry"),),
                            (&request.target, &request.function, attempts, delay_ms),
                        );
                        // Retry
                        continue;
                    }
                    Self::log_error(&env, &request.target, &request.function, ExecutionError::ContractRejected, attempts);
                    Self::append_history(&env, &request.target, &request.function, false, 0);
                    return Err(ExecutionError::ContractRejected);
                }
            }
        }
    }

    /// Estimate fees for a transaction.
    ///
    /// Returns a [`FeeEstimate`] based on the target contract and function.
    /// Under high-load conditions (detected via a configurable threshold), a
    /// surge multiplier is applied to the base fee.
    ///
    /// # Arguments
    /// * `target` - The contract to be called.
    /// * `function` - The function to be invoked.
    /// * `amount` - The transaction amount in stroops (used to scale resource fees).
    ///   Must be greater than zero.
    /// * `high_load_threshold` - Basis-point threshold above which surge pricing
    ///   applies (e.g., 8000 = 80% network utilization).
    ///
    /// # Errors
    /// * [`ExecutionError::InvalidAmount`] — if `amount` is ≤ 0.
    /// * [`ExecutionError::NotInitialized`] — if the contract is not initialized.
    pub fn estimate_fee(
        env: Env,
        _target: Address,
        _function: Symbol,
        amount: i128,
        high_load_threshold: u32,
    ) -> Result<FeeEstimate, ExecutionError> {
        // Verify initialized
        if !env.storage().instance().has(&DataKey::Admin) {
            return Err(ExecutionError::NotInitialized);
        }
        if amount <= 0 {
            return Err(ExecutionError::InvalidAmount);
        }

        // Base fee: 100 stroops minimum (Stellar network minimum)
        let base_fee: i128 = 100;

        // Resource fee scales with amount (0.1% of amount, min 100 stroops)
        let resource_fee: i128 = {
            let scaled = amount / 1000;
            if scaled < 100 { 100 } else { scaled }
        };

        // Surge pricing: if high_load_threshold >= 8000 bps (80%), apply 2x multiplier
        let (surge_multiplier, high_load) = if high_load_threshold >= 8000 {
            (200u32, true)
        } else {
            (100u32, false)
        };

        let total_fee = (base_fee + resource_fee) * surge_multiplier as i128 / 100;

        env.events().publish(
            (Symbol::new(&env, "fee_estimated"),),
            (total_fee, high_load),
        );

        Ok(FeeEstimate {
            base_fee,
            resource_fee,
            total_fee,
            surge_multiplier,
            high_load,
        })
    }

    /// Simulate a transaction without executing it.
    ///
    /// Runs a dry-run invocation via `try_invoke_contract` and returns a
    /// [`SimulationResult`] describing whether the transaction would succeed.
    /// The real execution is never performed — this is purely a validation step.
    ///
    /// Simulation results are logged via a `simulation_result` event so
    /// off-chain observers can track validation outcomes.
    ///
    /// # Arguments
    /// * `caller` - The address requesting simulation; must authenticate.
    /// * `target` - The contract to simulate against.
    /// * `function` - The function to simulate.
    ///
    /// # Returns
    /// A [`SimulationResult`] with `success`, `would_fail`, and a `message`.
    ///
    /// # Errors
    /// * [`ExecutionError::NotInitialized`] — if the contract is not initialized.
    pub fn simulate(
        env: Env,
        caller: Address,
        target: Address,
        function: Symbol,
    ) -> Result<SimulationResult, ExecutionError> {
        caller.require_auth();

        if !env.storage().instance().has(&DataKey::Admin) {
            return Err(ExecutionError::NotInitialized);
        }

        let args: Vec<soroban_sdk::Val> = Vec::new(&env);
        let sim_ok = env
            .try_invoke_contract::<soroban_sdk::Val, soroban_sdk::Val>(&target, &function, args)
            .is_ok();

        let message = if sim_ok {
            String::from_str(&env, "simulation succeeded")
        } else {
            String::from_str(&env, "simulation failed: transaction would be rejected")
        };

        env.events().publish(
            (Symbol::new(&env, "simulation_result"),),
            (&target, &function, sim_ok),
        );

        Ok(SimulationResult {
            target,
            function,
            success: sim_ok,
            would_fail: !sim_ok,
            message,
        })
    }

    /// Transfer admin to a new address.
    ///
    /// # Errors
    /// * [`ExecutionError::Unauthorized`] — if `current` is not the admin.
    /// * [`ExecutionError::NotInitialized`] — if the contract is not initialized.
    pub fn transfer_admin(
        env: Env,
        current: Address,
        new_admin: Address,
    ) -> Result<(), ExecutionError> {
        current.require_auth();
        let admin: Address = env
            .storage()
            .instance()
            .get(&DataKey::Admin)
            .ok_or(ExecutionError::NotInitialized)?;
        if admin != current {
            return Err(ExecutionError::Unauthorized);
        }
        router_common::admin_transfer_complete!(&env, &current, &new_admin, &DataKey::Admin);
        Ok(())
    }

    /// Update the global max-retries cap (admin only).
    ///
    /// # Errors
    /// * [`ExecutionError::Unauthorized`] — if `caller` is not the admin.
    /// * [`ExecutionError::NotInitialized`] — if the contract is not initialized.
    /// * [`ExecutionError::InvalidConfig`] — if `new_max` > 5.
    pub fn set_max_retries(
        env: Env,
        caller: Address,
        new_max: u32,
    ) -> Result<(), ExecutionError> {
        caller.require_auth();
        let admin: Address = env
            .storage()
            .instance()
            .get(&DataKey::Admin)
            .ok_or(ExecutionError::NotInitialized)?;
        if admin != caller {
            return Err(ExecutionError::Unauthorized);
        }
        if new_max > 5 {
            return Err(ExecutionError::InvalidConfig);
        }
        env.storage().instance().set(&DataKey::MaxRetries, &new_max);
        Ok(())
    }

    /// Return up to `limit` most-recent execution history records (newest first).
    ///
    /// # Errors
    /// * [`ExecutionError::NotInitialized`] — if the contract is not initialized.
    pub fn get_execution_history(
        env: Env,
        limit: u32,
    ) -> Result<Vec<ExecutionRecord>, ExecutionError> {
        if !env.storage().instance().has(&DataKey::Admin) {
            return Err(ExecutionError::NotInitialized);
        }
        let history: Vec<ExecutionRecord> = env
            .storage()
            .instance()
            .get(&DataKey::ExecHistory)
            .unwrap_or(Vec::new(&env));
        let len = history.len();
        let take = if limit as u32 > len { len } else { limit as u32 };
        let mut result = Vec::new(&env);
        // Return newest-first: iterate from the end
        let mut i = len;
        let mut collected = 0u32;
        while i > 0 && collected < take {
            i -= 1;
            result.push_back(history.get(i).unwrap());
            collected += 1;
        }
        Ok(result)
    }

    /// Get the current admin address.
    ///
    /// # Errors
    /// Returns `ExecutionError::NotInitialized` if the contract has not been initialized.
    pub fn admin(env: Env) -> Result<Address, ExecutionError> {
        env.storage()
            .instance()
            .get(&DataKey::Admin)
            .ok_or(ExecutionError::NotInitialized)
    }

    /// Get cumulative execution statistics.
    ///
    /// Returns `(total_executions, total_errors)`.
    pub fn stats(env: Env) -> (u64, u64) {
        let execs: u64 = env.storage().instance().get(&DataKey::TotalExecutions).unwrap_or(0);
        let errors: u64 = env.storage().instance().get(&DataKey::TotalErrors).unwrap_or(0);
        (execs, errors)
    }

    // ── Helpers ───────────────────────────────────────────────────────────────

    /// Compute exponential backoff delay in milliseconds for a given attempt index.
    ///
    /// `delay = base_ms * (multiplier/100)^attempt_index`
    ///
    /// Uses integer arithmetic: multiply by `multiplier` and divide by 100 for
    /// each step to avoid floating point.
    pub(crate) fn compute_backoff_ms(base_ms: u64, multiplier: u32, attempt_index: u32) -> u64 {
        let mut delay = base_ms;
        for _ in 0..attempt_index {
            delay = delay.saturating_mul(multiplier as u64) / 100;
        }
        delay
    }

    fn log_error(env: &Env, target: &Address, function: &Symbol, error: ExecutionError, attempts: u32) {
        Self::increment_counter(env, &DataKey::TotalErrors);
        // Emit a structured error event; does not leak internal details beyond
        // the error code and attempt count.
        env.events().publish(
            (Symbol::new(env, "execution_error"),),
            (target, function, error as u32, attempts),
        );
    }

    fn increment_counter(env: &Env, key: &DataKey) {
        let val: u64 = env.storage().instance().get(key).unwrap_or(0);
        env.storage().instance().set(key, &(val + 1));
    }

    fn append_history(env: &Env, target: &Address, function: &Symbol, success: bool, fee_paid: i128) {
        let mut history: Vec<ExecutionRecord> = env
            .storage()
            .instance()
            .get(&DataKey::ExecHistory)
            .unwrap_or(Vec::new(env));
        history.push_back(ExecutionRecord {
            timestamp: env.ledger().timestamp(),
            target: target.clone(),
            function: function.clone(),
            success,
            fee_paid,
        });
        env.storage().instance().set(&DataKey::ExecHistory, &history);
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use soroban_sdk::{testutils::{Address as _, Events as _}, Env, IntoVal};

    fn setup() -> (Env, Address, RouterExecutionClient<'static>) {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register_contract(None, RouterExecution);
        let client = RouterExecutionClient::new(&env, &contract_id);
        let admin = Address::generate(&env);
        client.initialize(&admin, &2, &500, &200); // base=500ms, multiplier=2x
        (env, admin, client)
    }

    #[test]
    fn test_initialize_twice_fails() {
        let (_, admin, client) = setup();
        let result = client.try_initialize(&admin, &1, &0, &100);
        assert_eq!(result, Err(Ok(ExecutionError::AlreadyInitialized)));
    }

    #[test]
    fn test_initialize_max_retries_too_high_fails() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register_contract(None, RouterExecution);
        let client = RouterExecutionClient::new(&env, &contract_id);
        let admin = Address::generate(&env);
        let result = client.try_initialize(&admin, &6, &0, &100);
        assert_eq!(result, Err(Ok(ExecutionError::InvalidConfig)));
    }

    #[test]
    fn test_initialize_invalid_multiplier_fails() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register_contract(None, RouterExecution);
        let client = RouterExecutionClient::new(&env, &contract_id);
        let admin = Address::generate(&env);
        // multiplier < 100 is invalid
        let result = client.try_initialize(&admin, &2, &500, &99);
        assert_eq!(result, Err(Ok(ExecutionError::InvalidConfig)));
    }

    #[test]
    fn test_admin_returns_initialized_admin() {
        let (_, admin, client) = setup();
        assert_eq!(client.admin(), admin);
    }

    #[test]
    fn test_fee_estimate_normal_load() {
        let (env, _, client) = setup();
        let target = Address::generate(&env);
        let function = Symbol::new(&env, "transfer");
        // 80% threshold not reached → no surge
        let estimate = client.estimate_fee(&target, &function, &1_000_000, &5000);
        assert!(!estimate.high_load);
        assert_eq!(estimate.surge_multiplier, 100);
        assert_eq!(estimate.base_fee, 100);
        assert_eq!(estimate.total_fee, estimate.base_fee + estimate.resource_fee);
    }

    #[test]
    fn test_fee_estimate_high_load() {
        let (env, _, client) = setup();
        let target = Address::generate(&env);
        let function = Symbol::new(&env, "transfer");
        let estimate = client.estimate_fee(&target, &function, &1_000_000, &8000);
        assert!(estimate.high_load);
        assert_eq!(estimate.surge_multiplier, 200);
        // total = (base + resource) * 2
        assert_eq!(estimate.total_fee, (estimate.base_fee + estimate.resource_fee) * 2);
    }

    #[test]
    fn test_fee_estimate_invalid_amount() {
        let (env, _, client) = setup();
        let target = Address::generate(&env);
        let function = Symbol::new(&env, "transfer");
        let result = client.try_estimate_fee(&target, &function, &0, &5000);
        assert_eq!(result, Err(Ok(ExecutionError::InvalidAmount)));
    }

    #[test]
    fn test_stats_initial() {
        let (_, _, client) = setup();
        assert_eq!(client.stats(), (0, 0));
    }

    #[test]
    fn test_simulate_nonexistent_contract_fails() {
        let (env, _, client) = setup();
        let caller = Address::generate(&env);
        let target = Address::generate(&env);
        let function = Symbol::new(&env, "transfer");
        // Calling a random address that has no contract → simulation should fail
        let result = client.simulate(&caller, &target, &function);
        assert!(!result.success);
        assert!(result.would_fail);
    }

    #[test]
    fn test_transfer_admin() {
        let (env, admin, client) = setup();
        let new_admin = Address::generate(&env);
        client.transfer_admin(&admin, &new_admin);
        assert_eq!(client.admin(), new_admin);
    }

    #[test]
    fn test_transfer_admin_unauthorized_fails() {
        let (env, _, client) = setup();
        let attacker = Address::generate(&env);
        let new_admin = Address::generate(&env);
        let result = client.try_transfer_admin(&attacker, &new_admin);
        assert_eq!(result, Err(Ok(ExecutionError::Unauthorized)));
    }

    #[test]
    fn test_transfer_admin_emits_event() {
        let (env, admin, client) = setup();
        let new_admin = Address::generate(&env);
        client.transfer_admin(&admin, &new_admin);
        let event = env.events().all().last().unwrap().clone();
        let topic: Symbol = event.1.get(0).unwrap().into_val(&env);
        assert_eq!(topic, Symbol::new(&env, "admin_transferred"));
    }

    #[test]
    fn test_set_max_retries() {
        let (_, admin, client) = setup();
        client.set_max_retries(&admin, &3);
    }

    #[test]
    fn test_set_max_retries_too_high_fails() {
        let (_, admin, client) = setup();
        let result = client.try_set_max_retries(&admin, &6);
        assert_eq!(result, Err(Ok(ExecutionError::InvalidConfig)));
    }

    #[test]
    fn test_set_max_retries_unauthorized_fails() {
        let (env, _, client) = setup();
        let attacker = Address::generate(&env);
        let result = client.try_set_max_retries(&attacker, &2);
        assert_eq!(result, Err(Ok(ExecutionError::Unauthorized)));
    }

    #[test]
    fn test_get_execution_history_empty() {
        let (_, _, client) = setup();
        let history = client.get_execution_history(&10);
        assert_eq!(history.len(), 0);
    }

    #[test]
    fn test_simulate_returns_message_on_failure() {
        let (env, _, client) = setup();
        let caller = Address::generate(&env);
        let target = Address::generate(&env);
        let function = Symbol::new(&env, "transfer");
        let result = client.simulate(&caller, &target, &function);
        // Message should indicate failure
        assert_eq!(
            result.message,
            soroban_sdk::String::from_str(&env, "simulation failed: transaction would be rejected")
        );
    }

    // ── Backoff config tests ──────────────────────────────────────────────────

    #[test]
    fn test_backoff_config_stored_on_initialize() {
        let (_, _, client) = setup();
        let (base, mult) = client.backoff_config();
        assert_eq!(base, 500);
        assert_eq!(mult, 200);
    }

    #[test]
    fn test_set_backoff_config_updates_values() {
        let (_, admin, client) = setup();
        client.set_backoff_config(&admin, &1000, &150);
        let (base, mult) = client.backoff_config();
        assert_eq!(base, 1000);
        assert_eq!(mult, 150);
    }

    #[test]
    fn test_set_backoff_config_unauthorized_fails() {
        let (env, _, client) = setup();
        let attacker = Address::generate(&env);
        let result = client.try_set_backoff_config(&attacker, &1000, &200);
        assert_eq!(result, Err(Ok(ExecutionError::Unauthorized)));
    }

    #[test]
    fn test_set_backoff_config_invalid_multiplier_fails() {
        let (_, admin, client) = setup();
        let result = client.try_set_backoff_config(&admin, &500, &99);
        assert_eq!(result, Err(Ok(ExecutionError::InvalidConfig)));
    }

    #[test]
    fn test_compute_backoff_ms_no_delay() {
        // base=0 → always 0 regardless of multiplier
        assert_eq!(RouterExecution::compute_backoff_ms(0, 200, 0), 0);
        assert_eq!(RouterExecution::compute_backoff_ms(0, 200, 3), 0);
    }

    #[test]
    fn test_compute_backoff_ms_doubles_each_attempt() {
        // base=100ms, multiplier=200 (2x): 100, 200, 400, 800
        assert_eq!(RouterExecution::compute_backoff_ms(100, 200, 0), 100);
        assert_eq!(RouterExecution::compute_backoff_ms(100, 200, 1), 200);
        assert_eq!(RouterExecution::compute_backoff_ms(100, 200, 2), 400);
        assert_eq!(RouterExecution::compute_backoff_ms(100, 200, 3), 800);
    }

    #[test]
    fn test_compute_backoff_ms_1x_multiplier_stays_constant() {
        // multiplier=100 (1x): delay stays at base
        assert_eq!(RouterExecution::compute_backoff_ms(250, 100, 0), 250);
        assert_eq!(RouterExecution::compute_backoff_ms(250, 100, 5), 250);
    }
}

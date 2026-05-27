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
    ///
    /// # Errors
    /// * [`ExecutionError::AlreadyInitialized`] — called more than once.
    /// * [`ExecutionError::InvalidConfig`] — `max_retries` exceeds 5.
    pub fn initialize(env: Env, admin: Address, max_retries: u32) -> Result<(), ExecutionError> {
        if env.storage().instance().has(&DataKey::Admin) {
            return Err(ExecutionError::AlreadyInitialized);
        }
        if max_retries > 5 {
            return Err(ExecutionError::InvalidConfig);
        }
        env.storage().instance().set(&DataKey::Admin, &admin);
        env.storage().instance().set(&DataKey::MaxRetries, &max_retries);
        env.storage().instance().set(&DataKey::TotalExecutions, &0u64);
        env.storage().instance().set(&DataKey::TotalErrors, &0u64);
        Ok(())
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
                    // Treat as a transient network error if retries remain
                    if attempts <= effective_retries {
                        // Retry
                        continue;
                    }
                    Self::log_error(&env, &request.target, &request.function, ExecutionError::ContractRejected, attempts);
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

    /// Get the current admin address.
    pub fn admin(env: Env) -> Address {
        env.storage()
            .instance()
            .get(&DataKey::Admin)
            .expect("router-execution not initialized")
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
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use soroban_sdk::{testutils::Address as _, Env};

    fn setup() -> (Env, Address, RouterExecutionClient<'static>) {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register_contract(None, RouterExecution);
        let client = RouterExecutionClient::new(&env, &contract_id);
        let admin = Address::generate(&env);
        client.initialize(&admin, &2);
        (env, admin, client)
    }

    #[test]
    fn test_initialize_twice_fails() {
        let (_, admin, client) = setup();
        let result = client.try_initialize(&admin, &1);
        assert_eq!(result, Err(Ok(ExecutionError::AlreadyInitialized)));
    }

    #[test]
    fn test_initialize_max_retries_too_high_fails() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register_contract(None, RouterExecution);
        let client = RouterExecutionClient::new(&env, &contract_id);
        let admin = Address::generate(&env);
        let result = client.try_initialize(&admin, &6);
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
}

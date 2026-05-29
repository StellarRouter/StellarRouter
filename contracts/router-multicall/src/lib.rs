#![no_std]

//! # router-multicall
//!
//! Batch multiple cross-contract read calls in a single transaction.
//! Reduces round-trips when a client needs data from multiple contracts.
//!
//! ## Features
//! - Aggregate up to N calls in one transaction
//! - Per-call success/failure tracking (non-atomic mode)
//! - Atomic mode: revert all if any call fails
//! - Call result storage for async inspection
//!
//! ## Events (following naming convention: past tense verbs in snake_case)
//! - `call_result` — Individual call result logged (caller, target, function, success)
//! - `batch_executed` — Batch execution completed (summary_data)
//! - `max_batch_size_updated` — Max batch size updated (old_size, new_size)
//! - `admin_transferred` — Admin transferred (old_admin, new_admin)

use soroban_sdk::{
    contract, contractimpl, contracttype, contracterror,
    Address, Env, Vec, Symbol, Val,
};

// ── Storage Keys ──────────────────────────────────────────────────────────────

#[contracttype]
pub enum DataKey {
    Admin,
    MaxBatchSize,
    TotalBatches,
    Executing, // reentrancy guard
    BatchResult(u64, u32), // (batch_id, call_index) -> CallResult
}

// ── Types ─────────────────────────────────────────────────────────────────────

/// A single call descriptor in a batch.
#[contracttype]
#[derive(Clone, Debug, PartialEq)]
pub struct CallDescriptor {
    /// Target contract address
    pub target: Address,
    /// Function name to call
    pub function: Symbol,
    /// Whether failure of this call should abort the whole batch
    pub required: bool,
    /// Optional CPU instruction budget for this call.
    ///
    /// NOTE: Soroban's host does not expose a per-call instruction counter to
    /// guest contracts at runtime. This field is reserved for future use when
    /// the host surfaces budget metering to contracts. Currently, any value set
    /// here is stored and reflected in events/summary but cannot be enforced
    /// mid-call. Budget overruns at the transaction level are still caught by
    /// the host and will cause the entire transaction to fail.
    pub instruction_budget: Option<u64>,
}

/// Result of a single call in a batch.
#[contracttype]
#[derive(Clone, Debug, PartialEq)]
pub struct CallResult {
    pub target: Address,
    pub function: Symbol,
    pub success: bool,
}

/// Summary of a batch execution.
#[contracttype]
#[derive(Clone, Debug, PartialEq)]
pub struct BatchSummary {
    pub total: u32,
    pub succeeded: u32,
    pub failed: u32,
    /// Number of calls that failed while an `instruction_budget` was set.
    ///
    /// Because the Soroban host does not currently expose a per-call CPU
    /// counter to guest contracts, this counts calls that *failed* and had a
    /// budget specified — a conservative proxy until host metering is
    /// surfaced to contracts.
    pub budget_exceeded_count: u32,
}

// ── Errors ────────────────────────────────────────────────────────────────────

#[contracterror]
#[derive(Copy, Clone, Debug, PartialEq)]
pub enum MulticallError {
    AlreadyInitialized = 1,
    NotInitialized = 2,
    Unauthorized = 3,
    BatchTooLarge = 4,
    EmptyBatch = 5,
    RequiredCallFailed = 6,
    InvalidConfig = 7,
    Reentrancy = 8,
}

// ── Contract ──────────────────────────────────────────────────────────────────

#[contract]
pub struct RouterMulticall;

#[contractimpl]
impl RouterMulticall {
    /// Initialize with admin and maximum batch size.
    ///
    /// Must be called exactly once. Sets the admin, the maximum number of calls
    /// allowed per batch, and resets the total batch counter to zero.
    ///
    /// # Arguments
    /// * `env` - The Soroban environment.
    /// * `admin` - The address that will have admin privileges over this contract.
    /// * `max_batch_size` - The maximum number of [`CallDescriptor`]s allowed in
    ///   a single `execute_batch` call. Must be greater than zero.
    ///
    /// # Returns
    /// `Ok(())` on success.
    ///
    /// # Errors
    /// * [`MulticallError::AlreadyInitialized`] — if the contract has already been initialized.
    /// * [`MulticallError::InvalidConfig`] — if `max_batch_size` is zero.
    pub fn initialize(env: Env, admin: Address, max_batch_size: u32) -> Result<(), MulticallError> {
        if env.storage().instance().has(&DataKey::Admin) {
            return Err(MulticallError::AlreadyInitialized);
        }
        if max_batch_size == 0 {
            return Err(MulticallError::InvalidConfig);
        }
        env.storage().instance().set(&DataKey::Admin, &admin);
        env.storage().instance().set(&DataKey::MaxBatchSize, &max_batch_size);
        env.storage().instance().set(&DataKey::TotalBatches, &0u64);
        Ok(())
    }

    /// Execute a batch of calls. Returns a summary of results.
    ///
    /// **Access Control:** This function can be called by ANY authenticated
    /// address, not just the admin. This is intentional — `router-multicall`
    /// is designed as a public batching service. Any caller can batch their
    /// own cross-contract calls to reduce round-trips. The admin role is only
    /// used for configuration (e.g., setting `max_batch_size`).
    ///
    /// Iterates over each [`CallDescriptor`] in `calls` and attempts a
    /// cross-contract invocation. Tracks per-call success and failure. If a
    /// call marked `required` fails, the entire batch is aborted and
    /// [`MulticallError::RequiredCallFailed`] is returned. On completion,
    /// increments the total batch counter (unless `simulate` is `true`).
    ///
    /// When `store_results` is `true`, each [`CallResult`] is persisted under
    /// `DataKey::BatchResult(batch_id, call_index)` for later inspection.
    ///
    /// # Arguments
    /// * `env` - The Soroban environment.
    /// * `caller` - The address initiating the batch; must authenticate.
    ///   Can be any address, not restricted to admin.
    /// * `calls` - A list of [`CallDescriptor`]s describing each call to make.
    ///   Must be non-empty and no larger than the configured `max_batch_size`.
    /// * `simulate` - If `true`, executes in dry-run mode: all calls are attempted
    ///   but the batch counter is not incremented.
    /// * `store_results` - If `true`, each [`CallResult`] is persisted under
    ///   `DataKey::BatchResult(batch_id, call_index)` for later inspection.
    ///
    /// # Returns
    /// A [`BatchSummary`] with the total, succeeded, failed, and budget_exceeded_count.
    ///
    /// # Errors
    /// * [`MulticallError::EmptyBatch`] — if `calls` is empty.
    /// * [`MulticallError::BatchTooLarge`] — if `calls` exceeds `max_batch_size`.
    /// * [`MulticallError::RequiredCallFailed`] — if a call with `required = true` fails.
    /// * [`MulticallError::NotInitialized`] — if the contract has not been initialized.
    pub fn execute_batch(
        env: Env,
        caller: Address,
        calls: Vec<CallDescriptor>,
        simulate: bool,
        store_results: bool,
    ) -> Result<BatchSummary, MulticallError> {
        caller.require_auth();

        // Reentrancy guard
        if env.storage().instance().get::<DataKey, bool>(&DataKey::Executing).unwrap_or(false) {
            return Err(MulticallError::Reentrancy);
        }
        env.storage().instance().set(&DataKey::Executing, &true);

        if calls.is_empty() {
            env.storage().instance().remove(&DataKey::Executing);
            return Err(MulticallError::EmptyBatch);
        }

        let max: u32 = match env.storage().instance().get(&DataKey::MaxBatchSize) {
            Some(v) => v,
            None => {
                env.storage().instance().remove(&DataKey::Executing);
                return Err(MulticallError::NotInitialized);
            }
        };

        if calls.len() > max {
            env.storage().instance().remove(&DataKey::Executing);
            return Err(MulticallError::BatchTooLarge);
        }

        let batch_id: u64 = env
            .storage()
            .instance()
            .get(&DataKey::TotalBatches)
            .unwrap_or(0);

        let mut succeeded = 0u32;
        let mut failed = 0u32;
        let mut budget_exceeded_count = 0u32;
        let total = calls.len();

        let mut call_index = 0u32;
        for call in calls.iter() {
            let args: Vec<Val> = Vec::new(&env);
            let result = env.try_invoke_contract::<Val, Val>(&call.target, &call.function, args);

            let success = result.is_ok();

            if success {
                succeeded += 1;
            } else {
                failed += 1;
                if call.instruction_budget.is_some() {
                    budget_exceeded_count += 1;
                }
            }

            if store_results {
                env.storage().instance().set(
                    &DataKey::BatchResult(batch_id, call_index),
                    &CallResult {
                        target: call.target.clone(),
                        function: call.function.clone(),
                        success,
                    },
                );
            }

            env.events().publish(
                (Symbol::new(&env, "call_result"),),
                (&caller, &call.target, &call.function, success),
            );

            if !success && call.required {
                env.storage().instance().remove(&DataKey::Executing);
                return Err(MulticallError::RequiredCallFailed);
            }

            call_index += 1;
        }

        if !simulate {
            env.storage().instance().set(&DataKey::TotalBatches, &(batch_id + 1));
        }

        env.storage().instance().remove(&DataKey::Executing);

        Ok(BatchSummary {
            total,
            succeeded,
            failed,
            budget_exceeded_count,
        })
    }

    /// Update the maximum batch size.
    ///
    /// Changes the upper limit on the number of calls allowed per batch.
    /// Caller must be the admin.
    ///
    /// # Arguments
    /// * `env` - The Soroban environment.
    /// * `caller` - The address initiating the call; must be the admin.
    /// * `max_batch_size` - The new maximum batch size. Must be greater than zero.
    ///
    /// # Returns
    /// `Ok(())` on success.
    ///
    /// # Errors
    /// * [`MulticallError::Unauthorized`] — if `caller` is not the admin.
    /// * [`MulticallError::InvalidConfig`] — if `max_batch_size` is zero.
    /// * [`MulticallError::NotInitialized`] — if the contract has not been initialized.
    pub fn set_max_batch_size(
        env: Env,
        caller: Address,
        max_batch_size: u32,
    ) -> Result<(), MulticallError> {
        caller.require_auth();
        Self::require_admin(&env, &caller)?;
        if max_batch_size == 0 {
            return Err(MulticallError::InvalidConfig);
        }
        let old_max: u32 = env.storage().instance()
            .get(&DataKey::MaxBatchSize)
            .unwrap_or(0);
        env.storage().instance().set(&DataKey::MaxBatchSize, &max_batch_size);
        env.events().publish(
            (Symbol::new(&env, "max_batch_size_updated"),),
            (old_max, max_batch_size),
        );
        Ok(())
    }

    /// Get total batches executed.
    ///
    /// Returns the cumulative count of successful `execute_batch`
    /// invocations since the contract was initialized.
    ///
    /// # Arguments
    /// * `env` - The Soroban environment.
    ///
    /// # Returns
    /// The total number of batches that have been executed.
    pub fn total_batches(env: Env) -> u64 {
        env.storage().instance().get(&DataKey::TotalBatches).unwrap_or(0)
    }

    /// Get the max batch size.
    ///
    /// # Arguments
    /// * `env` - The Soroban environment.
    ///
    /// # Returns
    /// The maximum number of calls allowed per batch.
    ///
    /// # Errors
    /// * [`MulticallError::NotInitialized`] — if the contract has not been initialized.
    pub fn max_batch_size(env: Env) -> Result<u32, MulticallError> {
        env.storage()
            .instance()
            .get(&DataKey::MaxBatchSize)
            .ok_or(MulticallError::NotInitialized)
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
    /// Note: This is a breaking change from the previous Result-based API.
    /// Calling admin() on an uninitialized contract is considered a programming error
    /// rather than a runtime condition, consistent with how similar getters work.
    pub fn admin(env: Env) -> Address {
        env.storage()
            .instance()
            .get(&DataKey::Admin)
            .expect("not initialized")
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
    /// * [`MulticallError::Unauthorized`] — if `current` is not the admin.
    /// * [`MulticallError::NotInitialized`] — if the contract has not been initialized.
    pub fn transfer_admin(env: Env, current: Address, new_admin: Address) -> Result<(), MulticallError> {
        current.require_auth();
        Self::require_admin(&env, &current)?;
        router_common::admin_transfer_complete!(&env, &current, &new_admin, &DataKey::Admin);
        Ok(())
    }

    // ── Helpers ───────────────────────────────────────────────────────────────

    fn require_admin(env: &Env, caller: &Address) -> Result<(), MulticallError> {
        let admin = Self::admin(env.clone());
        if &admin != caller {
            return Err(MulticallError::Unauthorized);
        }
        Ok(())
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    extern crate std;
    use super::*;
    use soroban_sdk::{testutils::{Address as _, Events}, Env, FromVal, Symbol, Vec};

    fn setup() -> (Env, Address, RouterMulticallClient<'static>) {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register_contract(None, RouterMulticall);
        let client = RouterMulticallClient::new(&env, &contract_id);
        let admin = Address::generate(&env);
        client.initialize(&admin, &10);
        (env, admin, client)
    }

    #[test]
    fn test_initialize() {
        let (_, _, client) = setup();
        assert_eq!(client.max_batch_size(), 10);
        assert_eq!(client.total_batches(), 0);
    }

    #[test]
    fn test_double_initialize_fails() {
        let (_env, admin, client) = setup();
        let result = client.try_initialize(&admin, &10);
        assert_eq!(result, Err(Ok(MulticallError::AlreadyInitialized)));
    }

    #[test]
    fn test_empty_batch_fails() {
        let (env, _admin, client) = setup();
        let caller = Address::generate(&env);
        let calls: Vec<CallDescriptor> = Vec::new(&env);
        let result = client.try_execute_batch(&caller, &calls, &false, &false);
        assert_eq!(result, Err(Ok(MulticallError::EmptyBatch)));
    }

    #[test]
    fn test_batch_too_large_fails() {
        let (env, admin, client) = setup();
        client.set_max_batch_size(&admin, &2);
        let caller = Address::generate(&env);
        let mut calls: Vec<CallDescriptor> = Vec::new(&env);
        for _ in 0..3 {
            calls.push_back(CallDescriptor {
                target: Address::generate(&env),
                function: Symbol::new(&env, "ping"),
                required: false,
                instruction_budget: None,
            });
        }
        let result = client.try_execute_batch(&caller, &calls, &false, &false);
        assert_eq!(result, Err(Ok(MulticallError::BatchTooLarge)));
    }

    #[test]
    fn test_set_max_batch_size() {
        let (_env, admin, client) = setup();
        client.set_max_batch_size(&admin, &5);
        assert_eq!(client.max_batch_size(), 5);
    }

    #[test]
    fn test_set_max_batch_size_emits_event() {
        let (env, admin, client) = setup();
        // initial max is 10 (from setup)
        client.set_max_batch_size(&admin, &5);
        let events = env.events().all();
        let last = events.last().unwrap();
        let topic: Symbol = last.1.get(0).unwrap().into_val(&env);
        assert_eq!(topic, Symbol::new(&env, "max_batch_size_updated"));
        let (old, new): (u32, u32) = last.2.into_val(&env);
        assert_eq!(old, 10);
        assert_eq!(new, 5);
    }

    #[test]
    fn test_unauthorized_set_max_fails() {
        let (env, _admin, client) = setup();
        let attacker = Address::generate(&env);
        let result = client.try_set_max_batch_size(&attacker, &5);
        assert_eq!(result, Err(Ok(MulticallError::Unauthorized)));
    }

    #[test]
    fn test_invalid_config_zero_max_fails() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register_contract(None, RouterMulticall);
        let client = RouterMulticallClient::new(&env, &contract_id);
        let admin = Address::generate(&env);
        let result = client.try_initialize(&admin, &0);
        assert_eq!(result, Err(Ok(MulticallError::InvalidConfig)));
    }

    #[contract]
    pub struct MockContract;

    #[contractimpl]
    impl MockContract {
        pub fn success(_env: Env) {}
        pub fn fail(_env: Env) {
            panic!("intended failure");
        }
    }

    #[test]
    fn test_all_calls_succeed() {
        let (env, _admin, client) = setup();
        let mock_id = env.register_contract(None, MockContract);
        let caller = Address::generate(&env);

        let mut calls = Vec::new(&env);
        calls.push_back(CallDescriptor {
            target: mock_id.clone(),
            function: Symbol::new(&env, "success"),
            required: true,
            instruction_budget: None,
        });
        calls.push_back(CallDescriptor {
            target: mock_id.clone(),
            function: Symbol::new(&env, "success"),
            required: false,
            instruction_budget: None,
        });

        let summary = client.execute_batch(&caller, &calls, &false, &false);
        assert_eq!(summary.total, 2);
        assert_eq!(summary.succeeded, 2);
        assert_eq!(summary.failed, 0);
        assert_eq!(summary.budget_exceeded_count, 0);
        assert_eq!(client.total_batches(), 1);
    }

    #[test]
    fn test_optional_calls_fail_batch_completes() {
        let (env, _admin, client) = setup();
        let mock_id = env.register_contract(None, MockContract);
        let caller = Address::generate(&env);

        let mut calls = Vec::new(&env);
        // Successful required call
        calls.push_back(CallDescriptor {
            target: mock_id.clone(),
            function: Symbol::new(&env, "success"),
            required: true,
            instruction_budget: None,
        });
        // Failing optional call
        calls.push_back(CallDescriptor {
            target: mock_id.clone(),
            function: Symbol::new(&env, "fail"),
            required: false,
            instruction_budget: None,
        });
        // Successful optional call
        calls.push_back(CallDescriptor {
            target: mock_id.clone(),
            function: Symbol::new(&env, "success"),
            required: false,
            instruction_budget: None,
        });

        let summary = client.execute_batch(&caller, &calls, &false, &false);
        assert_eq!(summary.total, 3);
        assert_eq!(summary.succeeded, 2);
        assert_eq!(summary.failed, 1);
        assert_eq!(client.total_batches(), 1);
    }

    #[test]
    fn test_required_call_fails_aborts_batch() {
        let (env, _admin, client) = setup();
        let mock_id = env.register_contract(None, MockContract);
        let caller = Address::generate(&env);

        let mut calls = Vec::new(&env);
        // Successful optional call
        calls.push_back(CallDescriptor {
            target: mock_id.clone(),
            function: Symbol::new(&env, "success"),
            required: false,
            instruction_budget: None,
        });
        // Failing required call
        calls.push_back(CallDescriptor {
            target: mock_id.clone(),
            function: Symbol::new(&env, "fail"),
            required: true,
            instruction_budget: None,
        });
        // This should not even reach
        calls.push_back(CallDescriptor {
            target: mock_id.clone(),
            function: Symbol::new(&env, "success"),
            required: false,
            instruction_budget: None,
        });

        let result = client.try_execute_batch(&caller, &calls, &false, &false);
        assert_eq!(result, Err(Ok(MulticallError::RequiredCallFailed)));
        // Total batches should NOT increment if it failed
        assert_eq!(client.total_batches(), 0);
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

        let events = env.events().all();
        let last = events.last().unwrap();
        let topic: Symbol = last.1.get(0).unwrap().into_val(&env);
        assert_eq!(topic, Symbol::new(&env, "admin_transferred"));
        let (event_old, event_new): (Address, Address) = last.2.into_val(&env);
        assert_eq!(event_old, admin);
        assert_eq!(event_new, new_admin);
    }

    #[test]
    fn test_unauthorized_transfer_admin_fails() {
        let (env, _admin, client) = setup();
        let attacker = Address::generate(&env);
        let new_admin = Address::generate(&env);
        let result = client.try_transfer_admin(&attacker, &new_admin);
        assert_eq!(result, Err(Ok(MulticallError::Unauthorized)));
    }

    #[test]
    fn test_old_admin_locked_out_after_transfer() {
        let (env, admin, client) = setup();
        let new_admin = Address::generate(&env);
        client.transfer_admin(&admin, &new_admin);

        // old admin should no longer be able to update admin-only config
        let result = client.try_set_max_batch_size(&admin, &5);
        assert_eq!(result, Err(Ok(MulticallError::Unauthorized)));

        // new admin should be able to update config
        assert!(client.try_set_max_batch_size(&new_admin, &5).is_ok());
        assert_eq!(client.max_batch_size(), 5);
    }

    #[test]
    fn test_budget_exceeded_count_increments_on_budgeted_failure() {
        let (env, _admin, client) = setup();
        let mock_id = env.register_contract(None, MockContract);
        let caller = Address::generate(&env);

        let mut calls = Vec::new(&env);
        // Failing call WITH a budget set — should count as budget_exceeded
        calls.push_back(CallDescriptor {
            target: mock_id.clone(),
            function: Symbol::new(&env, "fail"),
            required: false,
            instruction_budget: Some(500_000),
        });
        // Failing call WITHOUT a budget set — should NOT count
        calls.push_back(CallDescriptor {
            target: mock_id.clone(),
            function: Symbol::new(&env, "fail"),
            required: false,
            instruction_budget: None,
        });
        // Successful call with a budget — should NOT count
        calls.push_back(CallDescriptor {
            target: mock_id.clone(),
            function: Symbol::new(&env, "success"),
            required: false,
            instruction_budget: Some(500_000),
        });

        let summary = client.execute_batch(&caller, &calls, &false, &false);
        assert_eq!(summary.total, 3);
        assert_eq!(summary.succeeded, 1);
        assert_eq!(summary.failed, 2);
        assert_eq!(summary.budget_exceeded_count, 1);
    }

    #[test]
    fn test_simulate_mode_does_not_increment_counter() {
        let (env, _admin, client) = setup();
        let mock_id = env.register_contract(None, MockContract);
        let caller = Address::generate(&env);

        let mut calls = Vec::new(&env);
        calls.push_back(CallDescriptor {
            target: mock_id.clone(),
            function: Symbol::new(&env, "success"),
            required: true,
            instruction_budget: None,
        });

        let summary = client.execute_batch(&caller, &calls, &true, &false);
        assert_eq!(summary.total, 1);
        assert_eq!(summary.succeeded, 1);
        assert_eq!(summary.failed, 0);
        // Batch counter should NOT increment in simulate mode
        assert_eq!(client.total_batches(), 0);
    }

    #[test]
    fn test_simulate_mode_returns_correct_summary() {
        let (env, _admin, client) = setup();
        let mock_id = env.register_contract(None, MockContract);
        let caller = Address::generate(&env);

        let mut calls = Vec::new(&env);
        calls.push_back(CallDescriptor {
            target: mock_id.clone(),
            function: Symbol::new(&env, "success"),
            required: true,
            instruction_budget: None,
        });
        calls.push_back(CallDescriptor {
            target: mock_id.clone(),
            function: Symbol::new(&env, "fail"),
            required: false,
            instruction_budget: None,
        });

        let summary = client.execute_batch(&caller, &calls, &true, &false);
        assert_eq!(summary.total, 2);
        assert_eq!(summary.succeeded, 1);
        assert_eq!(summary.failed, 1);
    }

    #[test]
    fn test_optional_panic_increments_failure_count() {
        let (env, _admin, client) = setup();
        let mock_id = env.register_contract(None, MockContract);
        let caller = Address::generate(&env);

        let mut calls = Vec::new(&env);
        calls.push_back(CallDescriptor {
            target: mock_id.clone(),
            function: Symbol::new(&env, "success"),
            required: true,
            instruction_budget: None,
        });
        calls.push_back(CallDescriptor {
            target: mock_id.clone(),
            function: Symbol::new(&env, "fail"),
            required: false,
            instruction_budget: None,
        });

        let summary = client.execute_batch(&caller, &calls, &false, &false);
        assert_eq!(summary.total, 2);
        assert_eq!(summary.succeeded, 1);
        assert_eq!(summary.failed, 1);
    }

    #[test]
    fn test_call_result_event_includes_caller() {
            let (env, _admin, client) = setup();
            let mock_id = env.register_contract(None, MockContract);
            let caller = Address::generate(&env);

            let mut calls = Vec::new(&env);
            calls.push_back(CallDescriptor {
                target: mock_id.clone(),
                function: Symbol::new(&env, "success"),
                required: true,
                instruction_budget: None,
            });

            client.execute_batch(&caller, &calls, &false, &false);

            // Find the call_result event — tuple is (contract_id, topics: Vec<Val>, data: Val)
            let all_events = env.events().all();
            let (_, _, data) = all_events
                .iter()
                .find(|(_, topics, _)| {
                    topics
                        .get(0)
                        .map(|v| Symbol::from_val(&env, &v) == Symbol::new(&env, "call_result"))
                        .unwrap_or(false)
                })
                .expect("call_result event not found");

            // Data is a Vec<Val>; decode first element as Address and assert it equals caller
            let data_vec = soroban_sdk::Vec::<soroban_sdk::Val>::from_val(&env, &data);
            let event_caller = Address::from_val(&env, &data_vec.get(0).unwrap());

            assert_eq!(event_caller, caller);
        }

    #[test]
    fn test_total_batches_not_incremented_when_required_call_fails() {
        let (env, _admin, client) = setup();
        let mock_id = env.register_contract(None, MockContract);
        let caller = Address::generate(&env);

        let mut calls = Vec::new(&env);
        calls.push_back(CallDescriptor {
            target: mock_id.clone(),
            function: Symbol::new(&env, "fail"),
            required: true,
            instruction_budget: None,
        });

        let result = client.try_execute_batch(&caller, &calls, &false, &false);
        assert_eq!(result, Err(Ok(MulticallError::RequiredCallFailed)));
        assert_eq!(client.total_batches(), 0);
    }

    #[test]
    fn test_executing_flag_cleared_after_success() {
        let (env, _admin, client) = setup();
        let mock_id = env.register_contract(None, MockContract);
        let caller = Address::generate(&env);

        let mut calls = Vec::new(&env);
        calls.push_back(CallDescriptor {
            target: mock_id.clone(),
            function: Symbol::new(&env, "success"),
            required: true,
            instruction_budget: None,
        });

        client.execute_batch(&caller, &calls, &false, &false);

        // Flag must be cleared — a second call must succeed (not return Reentrancy)
        let result = client.try_execute_batch(&caller, &calls, &false, &false);
        assert!(result.is_ok());
    }

    #[test]
    fn test_executing_flag_cleared_after_required_failure() {
        let (env, _admin, client) = setup();
        let mock_id = env.register_contract(None, MockContract);
        let caller = Address::generate(&env);

        let mut fail_calls = Vec::new(&env);
        fail_calls.push_back(CallDescriptor {
            target: mock_id.clone(),
            function: Symbol::new(&env, "fail"),
            required: true,
            instruction_budget: None,
        });

        // First call fails
        let _ = client.try_execute_batch(&caller, &fail_calls, &false, &false);

        // Flag must be cleared — a subsequent call must not return Reentrancy
        let mut ok_calls = Vec::new(&env);
        ok_calls.push_back(CallDescriptor {
            target: mock_id.clone(),
            function: Symbol::new(&env, "success"),
            required: true,
            instruction_budget: None,
        });
        let result = client.try_execute_batch(&caller, &ok_calls, &false, &false);
        assert!(result.is_ok());
    }
}

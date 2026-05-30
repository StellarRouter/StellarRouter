#![no_std]

//! # router-timelock
//!
//! Delayed execution queue for sensitive router configuration changes.
//! Operations must wait a configurable minimum delay before execution.
//! Operations can be cancelled before execution.
//! Operations expire if not executed within `eta + grace_period_seconds`.
//!
//! ## Events (following naming convention: past tense verbs in snake_case)
//! - `op_queued`              — Operation queued (op_id, target, eta, grace_period_seconds)
//! - `op_executed`            — Operation executed (op_id, target)
//! - `op_cancelled`           — Operation cancelled (op_id)
//! - `op_description_updated` — Operation description updated (op_id, new_description)

use soroban_sdk::{
    contract, contracterror, contractimpl, contracttype, xdr::ToXdr, Address, Bytes, Env, String,
    Symbol, Vec,
};

// ── Storage Keys ──────────────────────────────────────────────────────────────

#[contracttype]
pub enum DataKey {
    Admin,
    MinDelay,
    Operation(u64), // legacy, unused
    NextOpId,       // legacy, unused
    FastTrackEnabled,
    OperationDeps(u64),      // legacy, unused
    EmergencyCouncil,        // legacy, unused
    RequiredApprovals,       // legacy, unused
    FastTrackApprovals(u64), // legacy, unused
    Op(Bytes),               // op_id -> Op
}

// ── Types ─────────────────────────────────────────────────────────────────────

#[contracttype]
#[derive(Clone, Debug, PartialEq)]
pub struct Op {
    pub proposer: Address,
    pub description: String,
    pub target: Address,
    pub eta: u64,
    /// Seconds after `eta` during which the operation may be executed.
    /// After `eta + grace_period_seconds` the operation is considered expired
    /// and can no longer be executed.
    pub grace_period_seconds: u64,
    pub executed: bool,
    pub cancelled: bool,
}

/// Human-readable status of a timelock operation.
#[contracttype]
#[derive(Clone, Debug, PartialEq)]
pub enum OperationStatus {
    /// Queued and waiting for ETA to elapse.
    Queued,
    /// ETA has elapsed, still within grace period, not yet executed.
    Ready,
    /// Successfully executed.
    Executed,
    /// Cancelled before execution.
    Cancelled,
    /// Grace period has elapsed without execution; operation can no longer be executed.
    Expired,
}

// ── Errors ────────────────────────────────────────────────────────────────────

#[contracterror]
#[derive(Copy, Clone, Debug, PartialEq)]
pub enum TimelockError {
    AlreadyInitialized = 1,
    NotInitialized = 2,
    Unauthorized = 3,
    NotFound = 4,
    NotReady = 5,
    AlreadyExecuted = 6,
    Cancelled = 7,
    DelayTooShort = 8,
    /// The grace period has elapsed; the operation can no longer be executed.
    Expired = 9,
}

// ── Contract ──────────────────────────────────────────────────────────────────

#[contract]
pub struct RouterTimelock;

#[contractimpl]
impl RouterTimelock {
    /// Initialize with an admin and minimum delay (seconds).
    pub fn initialize(env: Env, admin: Address, min_delay: u64) -> Result<(), TimelockError> {
        if env.storage().instance().has(&DataKey::Admin) {
            return Err(TimelockError::AlreadyInitialized);
        }
        env.storage().instance().set(&DataKey::Admin, &admin);
        env.storage().instance().set(&DataKey::MinDelay, &min_delay);
        Ok(())
    }

    /// Queue an operation. Returns the op_id (SHA-256 of description + target + eta).
    ///
    /// `grace_period_seconds` defines the window after `eta` during which the
    /// operation may be executed. Once `eta + grace_period_seconds` has elapsed
    /// the operation is considered expired and can no longer be executed.
    ///
    /// Emits `op_queued` with `(op_id, target, eta, grace_period_seconds)`.
    pub fn queue(
        env: Env,
        proposer: Address,
        description: String,
        target: Address,
        delay: u64,
        grace_period_seconds: u64,
        _deps: Vec<Bytes>,
    ) -> Result<Bytes, TimelockError> {
        proposer.require_auth();
        Self::require_admin(&env, &proposer)?;

        let min_delay: u64 = env
            .storage()
            .instance()
            .get(&DataKey::MinDelay)
            .ok_or(TimelockError::NotInitialized)?;

        if delay < min_delay {
            return Err(TimelockError::DelayTooShort);
        }

        let eta = env.ledger().timestamp() + delay;

        // Derive op_id from description bytes + target bytes + eta
        let mut preimage = Bytes::new(&env);
        preimage.append(&description.clone().to_xdr(&env));
        preimage.append(&target.clone().to_xdr(&env));
        let eta_bytes = eta.to_be_bytes();
        preimage.append(&Bytes::from_array(&env, &eta_bytes));

        let op_id: Bytes = env.crypto().sha256(&preimage).into();

        let op = Op {
            proposer,
            description,
            target: target.clone(),
            eta,
            grace_period_seconds,
            executed: false,
            cancelled: false,
        };
        env.storage()
            .instance()
            .set(&DataKey::Op(op_id.clone()), &op);

        env.events().publish(
            (Symbol::new(&env, "op_queued"),),
            (op_id.clone(), target, eta, grace_period_seconds),
        );

        Ok(op_id)
    }

    /// Cancel a queued operation before it is executed.
    pub fn cancel(env: Env, caller: Address, op_id: Bytes) -> Result<(), TimelockError> {
        caller.require_auth();
        Self::require_admin(&env, &caller)?;

        let mut op: Op = env
            .storage()
            .instance()
            .get(&DataKey::Op(op_id.clone()))
            .ok_or(TimelockError::NotFound)?;

        if op.executed {
            return Err(TimelockError::AlreadyExecuted);
        }
        if op.cancelled {
            return Err(TimelockError::Cancelled);
        }

        op.cancelled = true;
        env.storage()
            .instance()
            .set(&DataKey::Op(op_id.clone()), &op);

        env.events()
            .publish((Symbol::new(&env, "op_cancelled"),), op_id);

        Ok(())
    }

    /// Execute a queued operation after its ETA has passed and before its grace period expires.
    ///
    /// Returns `TimelockError::NotReady` if called before `eta`.
    /// Returns `TimelockError::Expired` if called after `eta + grace_period_seconds`.
    pub fn execute(env: Env, caller: Address, op_id: Bytes) -> Result<(), TimelockError> {
        caller.require_auth();
        Self::require_admin(&env, &caller)?;

        let mut op: Op = env
            .storage()
            .instance()
            .get(&DataKey::Op(op_id.clone()))
            .ok_or(TimelockError::NotFound)?;

        if op.cancelled {
            return Err(TimelockError::Cancelled);
        }
        if op.executed {
            return Err(TimelockError::AlreadyExecuted);
        }

        let now = env.ledger().timestamp();
        if now < op.eta {
            return Err(TimelockError::NotReady);
        }
        if now > op.eta + op.grace_period_seconds {
            return Err(TimelockError::Expired);
        }

        op.executed = true;
        env.storage()
            .instance()
            .set(&DataKey::Op(op_id.clone()), &op);

        env.events()
            .publish((Symbol::new(&env, "op_executed"),), (op_id, op.target));

        Ok(())
    }

    /// Update the description of a queued (not yet executed or cancelled) operation.
    ///
    /// Only the admin may call this. The operation must still be pending —
    /// descriptions of executed or cancelled operations cannot be changed.
    ///
    /// Emits `op_description_updated` with `(op_id, new_description)`.
    pub fn update_description(
        env: Env,
        caller: Address,
        op_id: Bytes,
        new_description: String,
    ) -> Result<(), TimelockError> {
        caller.require_auth();
        Self::require_admin(&env, &caller)?;

        let mut op: Op = env
            .storage()
            .instance()
            .get(&DataKey::Op(op_id.clone()))
            .ok_or(TimelockError::NotFound)?;

        if op.executed {
            return Err(TimelockError::AlreadyExecuted);
        }
        if op.cancelled {
            return Err(TimelockError::Cancelled);
        }

        op.description = new_description.clone();
        env.storage()
            .instance()
            .set(&DataKey::Op(op_id.clone()), &op);

        env.events().publish(
            (Symbol::new(&env, "op_description_updated"),),
            (op_id, new_description),
        );

        Ok(())
    }

    /// Get an operation by id.
    pub fn get_op(env: Env, op_id: Bytes) -> Option<Op> {
        env.storage().instance().get(&DataKey::Op(op_id))
    }

    /// Get the human-readable status of an operation.
    ///
    /// # Returns
    /// * `Cancelled` — if the operation was cancelled.
    /// * `Executed`  — if the operation was executed.
    /// * `Expired`   — if `now > eta + grace_period_seconds` (and not executed/cancelled).
    /// * `Ready`     — if `now >= eta` and still within the grace period.
    /// * `Queued`    — if `now < eta`.
    ///
    /// Returns `None` if no operation with `op_id` exists.
    pub fn get_operation_status(env: Env, op_id: Bytes) -> Option<OperationStatus> {
        let op: Op = env.storage().instance().get(&DataKey::Op(op_id))?;
        let now = env.ledger().timestamp();
        let status = if op.cancelled {
            OperationStatus::Cancelled
        } else if op.executed {
            OperationStatus::Executed
        } else if now > op.eta + op.grace_period_seconds {
            OperationStatus::Expired
        } else if now >= op.eta {
            OperationStatus::Ready
        } else {
            OperationStatus::Queued
        };
        Some(status)
    }

    /// Get the minimum delay.
    pub fn min_delay(env: Env) -> u64 {
        env.storage()
            .instance()
            .get(&DataKey::MinDelay)
            .unwrap_or(0)
    }

    /// Get the admin.
    pub fn admin(env: Env) -> Address {
        env.storage()
            .instance()
            .get(&DataKey::Admin)
            .expect("not initialized")
    }

    // ── Helpers ───────────────────────────────────────────────────────────────

    fn require_admin(env: &Env, caller: &Address) -> Result<(), TimelockError> {
        let admin: Address = env
            .storage()
            .instance()
            .get(&DataKey::Admin)
            .ok_or(TimelockError::NotInitialized)?;
        if &admin != caller {
            return Err(TimelockError::Unauthorized);
        }
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
        Bytes, Env, IntoVal, String,
    };

    /// Default grace period used in most tests: 24 hours.
    const GRACE: u64 = 86_400;

    fn setup() -> (Env, Address, RouterTimelockClient<'static>) {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register_contract(None, RouterTimelock);
        let client = RouterTimelockClient::new(&env, &contract_id);
        let admin = Address::generate(&env);
        client.initialize(&admin, &3600);
        (env, admin, client)
    }

    // ── queue ─────────────────────────────────────────────────────────────────

    #[test]
    fn test_queue_returns_op_id() {
        let (env, admin, client) = setup();
        let target = Address::generate(&env);
        let desc = String::from_str(&env, "upgrade oracle");
        let deps = Vec::new(&env);
        let op_id = client.queue(&admin, &desc, &target, &3600, &GRACE, &deps);
        assert!(!op_id.is_empty());
    }

    #[test]
    fn test_queue_emits_op_queued_event() {
        let (env, admin, client) = setup();
        let target = Address::generate(&env);
        let desc = String::from_str(&env, "upgrade oracle");
        let deps = Vec::new(&env);

        let op_id = client.queue(&admin, &desc, &target, &3600, &GRACE, &deps);

        let events = env.events().all();
        let last = events.last().unwrap();

        // Topic is "op_queued"
        let topic: Symbol = last.1.get(0).unwrap().into_val(&env);
        assert_eq!(topic, Symbol::new(&env, "op_queued"));

        // Payload is (op_id, target, eta, grace_period_seconds)
        let (emitted_id, emitted_target, emitted_eta, emitted_grace): (Bytes, Address, u64, u64) =
            last.2.into_val(&env);
        assert_eq!(emitted_id, op_id);
        assert_eq!(emitted_target, target);
        assert!(emitted_eta > 0);
        assert_eq!(emitted_grace, GRACE);
    }

    #[test]
    fn test_queue_stores_op() {
        let (env, admin, client) = setup();
        let target = Address::generate(&env);
        let desc = String::from_str(&env, "upgrade oracle");
        let deps = Vec::new(&env);

        let op_id = client.queue(&admin, &desc, &target, &3600, &GRACE, &deps);
        let op = client.get_op(&op_id).unwrap();

        assert_eq!(op.target, target);
        assert_eq!(op.grace_period_seconds, GRACE);
        assert!(!op.executed);
        assert!(!op.cancelled);
    }

    #[test]
    fn test_queue_stores_grace_period() {
        let (env, admin, client) = setup();
        let target = Address::generate(&env);
        let desc = String::from_str(&env, "check grace stored");
        let deps: Vec<Bytes> = Vec::new(&env);
        let custom_grace: u64 = 7200;

        let op_id = client.queue(&admin, &desc, &target, &3600, &custom_grace, &deps);
        let op = client.get_op(&op_id).unwrap();

        assert_eq!(op.grace_period_seconds, custom_grace);
    }

    // ── execute ───────────────────────────────────────────────────────────────

    #[test]
    fn test_execute_before_eta_fails() {
        let (env, admin, client) = setup();
        let target = Address::generate(&env);
        let desc = String::from_str(&env, "upgrade oracle");
        let deps = Vec::new(&env);

        let op_id = client.queue(&admin, &desc, &target, &3600, &GRACE, &deps);
        let result = client.try_execute(&admin, &op_id);
        assert_eq!(result, Err(Ok(TimelockError::NotReady)));
    }

    #[test]
    fn test_execute_after_eta_succeeds() {
        let (env, admin, client) = setup();
        let target = Address::generate(&env);
        let desc = String::from_str(&env, "upgrade oracle");
        let deps = Vec::new(&env);

        let op_id = client.queue(&admin, &desc, &target, &3600, &GRACE, &deps);
        env.ledger().with_mut(|l| l.timestamp += 3601);
        client.execute(&admin, &op_id);

        let op = client.get_op(&op_id).unwrap();
        assert!(op.executed);
    }

    #[test]
    fn test_execute_after_grace_period_fails() {
        let (env, admin, client) = setup();
        let target = Address::generate(&env);
        let desc = String::from_str(&env, "upgrade oracle");
        let deps: Vec<Bytes> = Vec::new(&env);
        let grace: u64 = 3600; // 1-hour grace window

        let op_id = client.queue(&admin, &desc, &target, &3600, &grace, &deps);
        // Jump past eta + grace_period_seconds
        env.ledger().with_mut(|l| l.timestamp += 3600 + grace + 1);
        let result = client.try_execute(&admin, &op_id);
        assert_eq!(result, Err(Ok(TimelockError::Expired)));
    }

    #[test]
    fn test_execute_at_grace_period_boundary_succeeds() {
        // Execution exactly at eta + grace_period_seconds is still valid (inclusive boundary).
        let (env, admin, client) = setup();
        let target = Address::generate(&env);
        let desc = String::from_str(&env, "boundary test");
        let deps: Vec<Bytes> = Vec::new(&env);
        let grace: u64 = 3600;

        let op_id = client.queue(&admin, &desc, &target, &3600, &grace, &deps);
        // Jump to exactly eta + grace_period_seconds
        env.ledger().with_mut(|l| l.timestamp += 3600 + grace);
        client.execute(&admin, &op_id);

        let op = client.get_op(&op_id).unwrap();
        assert!(op.executed);
    }

    #[test]
    fn test_execute_cancelled_op_fails() {
        let (env, admin, client) = setup();
        let target = Address::generate(&env);
        let desc = String::from_str(&env, "upgrade oracle");
        let deps = Vec::new(&env);

        let op_id = client.queue(&admin, &desc, &target, &3600, &GRACE, &deps);
        client.cancel(&admin, &op_id);
        env.ledger().with_mut(|l| l.timestamp += 3601);
        let result = client.try_execute(&admin, &op_id);
        assert_eq!(result, Err(Ok(TimelockError::Cancelled)));
    }

    #[test]
    fn test_execute_twice_fails() {
        let (env, admin, client) = setup();
        let target = Address::generate(&env);
        let desc = String::from_str(&env, "upgrade oracle");
        let deps = Vec::new(&env);

        let op_id = client.queue(&admin, &desc, &target, &3600, &GRACE, &deps);
        env.ledger().with_mut(|l| l.timestamp += 3601);
        client.execute(&admin, &op_id);
        let result = client.try_execute(&admin, &op_id);
        assert_eq!(result, Err(Ok(TimelockError::AlreadyExecuted)));
    }

    #[test]
    fn test_execute_emits_op_executed_event() {
        let (env, admin, client) = setup();
        let target = Address::generate(&env);
        let desc = String::from_str(&env, "upgrade oracle");
        let deps = Vec::new(&env);

        let op_id = client.queue(&admin, &desc, &target, &3600, &GRACE, &deps);
        env.ledger().with_mut(|l| l.timestamp += 3601);
        client.execute(&admin, &op_id);

        let events = env.events().all();
        let last = events.last().unwrap();
        let topic: Symbol = last.1.get(0).unwrap().into_val(&env);
        assert_eq!(topic, Symbol::new(&env, "op_executed"));
    }

    // ── cancel ────────────────────────────────────────────────────────────────

    #[test]
    fn test_cancel_op() {
        let (env, admin, client) = setup();
        let target = Address::generate(&env);
        let desc = String::from_str(&env, "upgrade oracle");
        let deps = Vec::new(&env);

        let op_id = client.queue(&admin, &desc, &target, &3600, &GRACE, &deps);
        client.cancel(&admin, &op_id);

        let op = client.get_op(&op_id).unwrap();
        assert!(op.cancelled);
    }

    #[test]
    fn test_cancel_emits_op_cancelled_event() {
        let (env, admin, client) = setup();
        let target = Address::generate(&env);
        let desc = String::from_str(&env, "upgrade oracle");
        let deps = Vec::new(&env);

        let op_id = client.queue(&admin, &desc, &target, &3600, &GRACE, &deps);
        client.cancel(&admin, &op_id);

        let events = env.events().all();
        let last = events.last().unwrap();
        let topic: Symbol = last.1.get(0).unwrap().into_val(&env);
        assert_eq!(topic, Symbol::new(&env, "op_cancelled"));
    }

    // ── validation ────────────────────────────────────────────────────────────

    #[test]
    fn test_delay_too_short_fails() {
        let (env, admin, client) = setup();
        let target = Address::generate(&env);
        let desc = String::from_str(&env, "upgrade oracle");
        let deps = Vec::new(&env);
        // min_delay is 3600, passing 100 should fail
        let result = client.try_queue(&admin, &desc, &target, &100, &GRACE, &deps);
        assert_eq!(result, Err(Ok(TimelockError::DelayTooShort)));
    }

    #[test]
    fn test_unauthorized_queue_fails() {
        let (env, _admin, client) = setup();
        let attacker = Address::generate(&env);
        let target = Address::generate(&env);
        let desc = String::from_str(&env, "upgrade oracle");
        let deps = Vec::new(&env);
        let result = client.try_queue(&attacker, &desc, &target, &3600, &GRACE, &deps);
        assert_eq!(result, Err(Ok(TimelockError::Unauthorized)));
    }

    // ── get_operation_status ──────────────────────────────────────────────────

    #[test]
    fn test_get_operation_status_queued() {
        let (env, admin, client) = setup();
        let target = Address::generate(&env);
        let desc = String::from_str(&env, "upgrade oracle");
        let deps = Vec::new(&env);
        let op_id = client.queue(&admin, &desc, &target, &3600, &GRACE, &deps);
        assert_eq!(client.get_operation_status(&op_id), Some(OperationStatus::Queued));
    }

    #[test]
    fn test_get_operation_status_ready() {
        let (env, admin, client) = setup();
        let target = Address::generate(&env);
        let desc = String::from_str(&env, "upgrade oracle");
        let deps = Vec::new(&env);
        let op_id = client.queue(&admin, &desc, &target, &3600, &GRACE, &deps);
        // Past ETA but still within grace period
        env.ledger().with_mut(|l| l.timestamp += 3601);
        assert_eq!(client.get_operation_status(&op_id), Some(OperationStatus::Ready));
    }

    #[test]
    fn test_get_operation_status_executed() {
        let (env, admin, client) = setup();
        let target = Address::generate(&env);
        let desc = String::from_str(&env, "upgrade oracle");
        let deps = Vec::new(&env);
        let op_id = client.queue(&admin, &desc, &target, &3600, &GRACE, &deps);
        env.ledger().with_mut(|l| l.timestamp += 3601);
        client.execute(&admin, &op_id);
        assert_eq!(client.get_operation_status(&op_id), Some(OperationStatus::Executed));
    }

    #[test]
    fn test_get_operation_status_cancelled() {
        let (env, admin, client) = setup();
        let target = Address::generate(&env);
        let desc = String::from_str(&env, "upgrade oracle");
        let deps = Vec::new(&env);
        let op_id = client.queue(&admin, &desc, &target, &3600, &GRACE, &deps);
        client.cancel(&admin, &op_id);
        assert_eq!(client.get_operation_status(&op_id), Some(OperationStatus::Cancelled));
    }

    #[test]
    fn test_get_operation_status_expired() {
        let (env, admin, client) = setup();
        let target = Address::generate(&env);
        let desc = String::from_str(&env, "upgrade oracle");
        let deps: Vec<Bytes> = Vec::new(&env);
        let grace: u64 = 3600;

        let op_id = client.queue(&admin, &desc, &target, &3600, &grace, &deps);
        // Jump past eta + grace_period_seconds
        env.ledger().with_mut(|l| l.timestamp += 3600 + grace + 1);
        assert_eq!(client.get_operation_status(&op_id), Some(OperationStatus::Expired));
    }

    #[test]
    fn test_get_operation_status_nonexistent_returns_none() {
        let (env, _admin, client) = setup();
        let fake_id = Bytes::from_array(&env, &[0u8; 32]);
        assert_eq!(client.get_operation_status(&fake_id), None);
    }

    // ── update_description ────────────────────────────────────────────────────

    #[test]
    fn test_update_description_succeeds() {
        let (env, admin, client) = setup();
        let target = Address::generate(&env);
        let deps = Vec::new(&env);

        let op_id = client.queue(&admin, &String::from_str(&env, "initial desc"), &target, &3600, &GRACE, &deps);
        let new_desc = String::from_str(&env, "corrected desc");
        client.update_description(&admin, &op_id, &new_desc);

        let op = client.get_op(&op_id).unwrap();
        assert_eq!(op.description, new_desc);
    }

    #[test]
    fn test_update_description_emits_event() {
        let (env, admin, client) = setup();
        let target = Address::generate(&env);
        let deps = Vec::new(&env);

        let op_id = client.queue(&admin, &String::from_str(&env, "initial desc"), &target, &3600, &GRACE, &deps);
        let new_desc = String::from_str(&env, "corrected desc");
        client.update_description(&admin, &op_id, &new_desc);

        let events = env.events().all();
        let last = events.last().unwrap();

        let topic: Symbol = last.1.get(0).unwrap().into_val(&env);
        assert_eq!(topic, Symbol::new(&env, "op_description_updated"));

        let (emitted_id, emitted_desc): (Bytes, String) = last.2.into_val(&env);
        assert_eq!(emitted_id, op_id);
        assert_eq!(emitted_desc, new_desc);
    }

    #[test]
    fn test_update_description_on_executed_op_fails() {
        let (env, admin, client) = setup();
        let target = Address::generate(&env);
        let deps = Vec::new(&env);

        let op_id = client.queue(&admin, &String::from_str(&env, "initial desc"), &target, &3600, &GRACE, &deps);
        env.ledger().with_mut(|l| l.timestamp += 3601);
        client.execute(&admin, &op_id);

        let result = client.try_update_description(
            &admin,
            &op_id,
            &String::from_str(&env, "too late"),
        );
        assert_eq!(result, Err(Ok(TimelockError::AlreadyExecuted)));
    }

    #[test]
    fn test_update_description_on_cancelled_op_fails() {
        let (env, admin, client) = setup();
        let target = Address::generate(&env);
        let deps = Vec::new(&env);

        let op_id = client.queue(&admin, &String::from_str(&env, "initial desc"), &target, &3600, &GRACE, &deps);
        client.cancel(&admin, &op_id);

        let result = client.try_update_description(
            &admin,
            &op_id,
            &String::from_str(&env, "too late"),
        );
        assert_eq!(result, Err(Ok(TimelockError::Cancelled)));
    }

    #[test]
    fn test_update_description_nonexistent_op_fails() {
        let (env, admin, client) = setup();
        let fake_id = Bytes::from_array(&env, &[0u8; 32]);

        let result = client.try_update_description(
            &admin,
            &fake_id,
            &String::from_str(&env, "ghost op"),
        );
        assert_eq!(result, Err(Ok(TimelockError::NotFound)));
    }

    #[test]
    fn test_update_description_unauthorized_fails() {
        let (env, admin, client) = setup();
        let attacker = Address::generate(&env);
        let target = Address::generate(&env);
        let deps: Vec<Bytes> = Vec::new(&env);

        let op_id = client.queue(
            &admin,
            &String::from_str(&env, "initial desc"),
            &target,
            &3600,
            &GRACE,
            &deps,
        );

        let result = client.try_update_description(
            &attacker,
            &op_id,
            &String::from_str(&env, "hacked"),
        );
        assert_eq!(result, Err(Ok(TimelockError::Unauthorized)));
    }

    #[test]
    fn test_update_description_ready_op_succeeds() {
        // An op that is past its ETA but not yet executed is still pending — update should work.
        let (env, admin, client) = setup();
        let target = Address::generate(&env);
        let deps = Vec::new(&env);

        let op_id = client.queue(&admin, &String::from_str(&env, "initial desc"), &target, &3600, &GRACE, &deps);
        env.ledger().with_mut(|l| l.timestamp += 3601);

        let new_desc = String::from_str(&env, "clarified before execution");
        client.update_description(&admin, &op_id, &new_desc);

        let op = client.get_op(&op_id).unwrap();
        assert_eq!(op.description, new_desc);
    }
}

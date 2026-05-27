#![no_std]

//! # router-timelock
//!
//! Delayed execution queue for sensitive router configuration changes.
//! Operations must wait a configurable minimum delay before execution.
//! Operations can be cancelled before execution.
//!
//! ## Events (following naming convention: past tense verbs in snake_case)
//! - `op_queued` — Operation queued (op_id, target, eta)
//! - `op_executed` — Operation executed (op_id, target)
//! - `op_cancelled` — Operation cancelled (op_id)

use soroban_sdk::{
    contract, contracterror, contractimpl, contracttype, crypto::Hash, Address, Bytes, Env,
    String, Symbol, Vec,
};

// ── Storage Keys ──────────────────────────────────────────────────────────────

#[contracttype]
pub enum DataKey {
    Admin,
    MinDelay,
    Operation(u64), // op_id -> TimelockOp
    NextOpId,
    FastTrackEnabled,
    OperationDeps(u64),      // op_id -> Vec<u64>
    EmergencyCouncil,        // Vec<Address>
    RequiredApprovals,       // u32 (M in M-of-N)
    FastTrackApprovals(u64), // op_id -> Vec<Address> (who has approved)
    Op(Bytes), // op_id -> Op
}

// ── Types ─────────────────────────────────────────────────────────────────────

#[contracttype]
#[derive(Clone, Debug, PartialEq)]
pub struct Op {
    pub proposer: Address,
    pub description: String,
    pub target: Address,
    pub eta: u64,
    pub executed: bool,
    pub cancelled: bool,
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
    /// Emits `op_queued` with `(op_id, target, eta)`.
    pub fn queue(
        env: Env,
        proposer: Address,
        description: String,
        target: Address,
        delay: u64,
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
        preimage.append(&description.to_bytes());
        preimage.append(&target.clone().to_xdr(&env));
        let eta_bytes = eta.to_be_bytes();
        preimage.append(&Bytes::from_array(&env, &eta_bytes));

        let op_id: Bytes = env.crypto().sha256(&preimage).into();

        let op = Op {
            proposer,
            description,
            target: target.clone(),
            eta,
            executed: false,
            cancelled: false,
        };
        env.storage()
            .instance()
            .set(&DataKey::Op(op_id.clone()), &op);

        env.events().publish(
            (Symbol::new(&env, "op_queued"),),
            (op_id.clone(), target, eta),
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

    /// Execute a queued operation after its ETA has passed.
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
        if env.ledger().timestamp() < op.eta {
            return Err(TimelockError::NotReady);
        }

        op.executed = true;
        env.storage()
            .instance()
            .set(&DataKey::Op(op_id.clone()), &op);

        env.events()
            .publish((Symbol::new(&env, "op_executed"),), (op_id, op.target));

        Ok(())
    }

    /// Get an operation by id.
    pub fn get_op(env: Env, op_id: Bytes) -> Option<Op> {
        env.storage().instance().get(&DataKey::Op(op_id))
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

    fn setup() -> (Env, Address, RouterTimelockClient<'static>) {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register_contract(None, RouterTimelock);
        let client = RouterTimelockClient::new(&env, &contract_id);
        let admin = Address::generate(&env);
        client.initialize(&admin, &3600);
        (env, admin, client)
    }

    #[test]
    fn test_queue_returns_op_id() {
        let (env, admin, client) = setup();
        let target = Address::generate(&env);
        let desc = String::from_str(&env, "upgrade oracle");
        let deps = Vec::new(&env);
        let op_id = client.queue(&admin, &desc, &target, &3600, &deps);
        assert!(!op_id.is_empty());
    }

    #[test]
    fn test_queue_emits_op_queued_event() {
        let (env, admin, client) = setup();
        let target = Address::generate(&env);
        let desc = String::from_str(&env, "upgrade oracle");
        let deps = Vec::new(&env);

        let op_id = client.queue(&admin, &desc, &target, &3600, &deps);

        let events = env.events().all();
        let last = events.last().unwrap();

        // Topic is "op_queued"
        let topic: Symbol = last.1.get(0).unwrap().into_val(&env);
        assert_eq!(topic, Symbol::new(&env, "op_queued"));

        // Payload is (op_id, target, eta)
        let (emitted_id, emitted_target, emitted_eta): (Bytes, Address, u64) =
            last.2.into_val(&env);
        assert_eq!(emitted_id, op_id);
        assert_eq!(emitted_target, target);
        assert!(emitted_eta > 0);
    }

    #[test]
    fn test_queue_stores_op() {
        let (env, admin, client) = setup();
        let target = Address::generate(&env);
        let desc = String::from_str(&env, "upgrade oracle");
        let deps = Vec::new(&env);

        let op_id = client.queue(&admin, &desc, &target, &3600, &deps);
        let op = client.get_op(&op_id).unwrap();

        assert_eq!(op.target, target);
        assert!(!op.executed);
        assert!(!op.cancelled);
    }

    #[test]
    fn test_execute_before_eta_fails() {
        let (env, admin, client) = setup();
        let target = Address::generate(&env);
        let desc = String::from_str(&env, "upgrade oracle");
        let deps = Vec::new(&env);

        let op_id = client.queue(&admin, &desc, &target, &3600, &deps);
        let result = client.try_execute(&admin, &op_id);
        assert_eq!(result, Err(Ok(TimelockError::NotReady)));
    }

    #[test]
    fn test_execute_after_eta_succeeds() {
        let (env, admin, client) = setup();
        let target = Address::generate(&env);
        let desc = String::from_str(&env, "upgrade oracle");
        let deps = Vec::new(&env);

        let op_id = client.queue(&admin, &desc, &target, &3600, &deps);
        env.ledger().with_mut(|l| l.timestamp += 3601);
        client.execute(&admin, &op_id);

        let op = client.get_op(&op_id).unwrap();
        assert!(op.executed);
    }

    #[test]
    fn test_cancel_op() {
        let (env, admin, client) = setup();
        let target = Address::generate(&env);
        let desc = String::from_str(&env, "upgrade oracle");
        let deps = Vec::new(&env);

        let op_id = client.queue(&admin, &desc, &target, &3600, &deps);
        client.cancel(&admin, &op_id);

        let op = client.get_op(&op_id).unwrap();
        assert!(op.cancelled);
    }

    #[test]
    fn test_execute_cancelled_op_fails() {
        let (env, admin, client) = setup();
        let target = Address::generate(&env);
        let desc = String::from_str(&env, "upgrade oracle");
        let deps = Vec::new(&env);

        let op_id = client.queue(&admin, &desc, &target, &3600, &deps);
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

        let op_id = client.queue(&admin, &desc, &target, &3600, &deps);
        env.ledger().with_mut(|l| l.timestamp += 3601);
        client.execute(&admin, &op_id);
        let result = client.try_execute(&admin, &op_id);
        assert_eq!(result, Err(Ok(TimelockError::AlreadyExecuted)));
    }

    #[test]
    fn test_delay_too_short_fails() {
        let (env, admin, client) = setup();
        let target = Address::generate(&env);
        let desc = String::from_str(&env, "upgrade oracle");
        let deps = Vec::new(&env);
        // min_delay is 3600, passing 100 should fail
        let result = client.try_queue(&admin, &desc, &target, &100, &deps);
        assert_eq!(result, Err(Ok(TimelockError::DelayTooShort)));
    }

    #[test]
    fn test_unauthorized_queue_fails() {
        let (env, _admin, client) = setup();
        let attacker = Address::generate(&env);
        let target = Address::generate(&env);
        let desc = String::from_str(&env, "upgrade oracle");
        let deps = Vec::new(&env);
        let result = client.try_queue(&attacker, &desc, &target, &3600, &deps);
        assert_eq!(result, Err(Ok(TimelockError::Unauthorized)));
    }

    #[test]
    fn test_execute_emits_op_executed_event() {
        let (env, admin, client) = setup();
        let target = Address::generate(&env);
        let desc = String::from_str(&env, "upgrade oracle");
        let deps = Vec::new(&env);

        let op_id = client.queue(&admin, &desc, &target, &3600, &deps);
        env.ledger().with_mut(|l| l.timestamp += 3601);
        client.execute(&admin, &op_id);

        let events = env.events().all();
        let last = events.last().unwrap();
        let topic: Symbol = last.1.get(0).unwrap().into_val(&env);
        assert_eq!(topic, Symbol::new(&env, "op_executed"));
    }

    #[test]
    fn test_cancel_emits_op_cancelled_event() {
        let (env, admin, client) = setup();
        let target = Address::generate(&env);
        let desc = String::from_str(&env, "upgrade oracle");
        let deps = Vec::new(&env);

        let op_id = client.queue(&admin, &desc, &target, &3600, &deps);
        client.cancel(&admin, &op_id);

        let events = env.events().all();
        let last = events.last().unwrap();
        let topic: Symbol = last.1.get(0).unwrap().into_val(&env);
        assert_eq!(topic, Symbol::new(&env, "op_cancelled"));
    }
}

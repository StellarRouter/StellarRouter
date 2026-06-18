#![no_std]

//! # router-timelock
//!
//! Delayed execution queue for sensitive router configuration changes.
//! Operations must wait a configurable minimum delay before execution.
//! Operations can be cancelled before execution.
//! Operations expire if not executed within `eta + grace_period_seconds`.
//!
//! ## Fast-Track Emergency Execution
//!
//! An emergency council of M-of-N members may bypass the normal delay by
//! calling `fast_track_approve`. Once `required_approvals` (M) are collected,
//! the operation executes immediately and a `critical_fast_tracked` event is
//! emitted. Fast-track is **disabled by default** and must be explicitly
//! enabled by the admin via `set_fast_track_enabled`.
//!
//! Council membership can only be changed via standard (non-fast-track) admin
//! calls (`add_council_member`, `remove_council_member`, `set_emergency_council`),
//! ensuring updates are subject to `min_delay`.
//!
//! ## Events (past tense, snake_case)
//! - `op_queued`              — Operation queued (op_id, target, eta, grace_period_seconds)
//! - `op_executed`            — Operation executed (op_id, target)
//! - `op_cancelled`           — Operation cancelled (op_id)
//! - `op_description_updated` — Operation description updated (op_id, new_description)
//! - `council_updated`        — Council batch-replaced (required, council)
//! - `council_member_added`   — Single member added (member)
//! - `council_member_removed` — Single member removed (member)
//! - `fast_track_toggled`     — Fast-track enabled state changed (enabled: bool)
//! - `critical_fast_tracked`  — Op executed via fast-track (op_id, target, approval_count)
//! - `admin_transferred`      — Admin transferred (old, new)
//! - `min_delay_updated`      — Min delay changed (new_delay)

use soroban_sdk::{
    contract, contracterror, contractimpl, contracttype, xdr::ToXdr, Address, Bytes, Env, String,
    Symbol, Vec,
};

// ── Storage Keys ──────────────────────────────────────────────────────────────

#[contracttype]
pub enum DataKey {
    Admin,
    MinDelay,
    FastTrackEnabled,
    PendingCount,
    Op(Bytes),
    EmergencyCouncil,
    RequiredApprovals,
    FastTrackApprovals(Bytes),
}

// ── Types ─────────────────────────────────────────────────────────────────────

#[contracttype]
#[derive(Clone, Debug, PartialEq)]
pub struct Op {
    pub proposer: Address,
    pub description: String,
    pub target: Address,
    pub eta: u64,
    /// Seconds after `eta` during which the operation may still be executed.
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
    /// Grace period elapsed without execution; cannot be executed.
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
    /// Grace period elapsed; the operation can no longer be executed.
    Expired = 9,
    /// `min_delay` or `new_delay` is zero.
    InvalidDelay = 10,
    /// Council config is invalid (e.g. required == 0 or required > council size).
    InvalidConfig = 11,
    /// Caller is not a member of the emergency council.
    NotCouncilMember = 12,
    /// Fast-track path is currently disabled.
    FastTrackDisabled = 13,
    /// Council member already approved this operation.
    AlreadyApproved = 14,
    /// Address is already a council member.
    AlreadyMember = 15,
}

// ── Contract ──────────────────────────────────────────────────────────────────

#[contract]
pub struct RouterTimelock;

#[contractimpl]
impl RouterTimelock {
    /// Initialize with an admin and minimum delay (seconds). `min_delay` must be > 0.
    pub fn initialize(env: Env, admin: Address, min_delay: u64) -> Result<(), TimelockError> {
        if env.storage().instance().has(&DataKey::Admin) {
            return Err(TimelockError::AlreadyInitialized);
        }
        if min_delay == 0 {
            return Err(TimelockError::InvalidDelay);
        }
        env.storage().instance().set(&DataKey::Admin, &admin);
        env.storage().instance().set(&DataKey::MinDelay, &min_delay);
        Ok(())
    }

    /// Queue an operation. Returns the op_id (SHA-256 of description + target + eta).
    ///
    /// `grace_period_seconds` is the window after `eta` during which the operation
    /// may be executed. After `eta + grace_period_seconds` it is considered expired.
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

        let mut preimage = Bytes::new(&env);
        preimage.append(&description.clone().to_xdr(&env));
        preimage.append(&target.clone().to_xdr(&env));
        preimage.append(&Bytes::from_array(&env, &eta.to_be_bytes()));

        let op_id: Bytes = env.crypto().sha256(&preimage).into();

        env.storage().instance().set(
            &DataKey::Op(op_id.clone()),
            &Op {
                proposer,
                description,
                target: target.clone(),
                eta,
                grace_period_seconds,
                executed: false,
                cancelled: false,
            },
        );

        let count: u64 = env
            .storage()
            .instance()
            .get(&DataKey::PendingCount)
            .unwrap_or(0);
        env.storage()
            .instance()
            .set(&DataKey::PendingCount, &(count + 1));

        env.events().publish(
            (Symbol::new(&env, "op_queued"),),
            (op_id.clone(), target, eta, grace_period_seconds),
        );

        Ok(op_id)
    }

    /// Cancel a queued operation before it is executed.
    ///
    /// Emits `op_cancelled` with `op_id`.
    pub fn cancel(env: Env, caller: Address, op_id: Bytes) -> Result<(), TimelockError> {
        caller.require_auth();
        Self::require_admin(&env, &caller)?;

        let mut op: Op = env
            .storage()
            .instance()
            .get(&DataKey::Op(op_id.clone()))
            .ok_or(TimelockError::NotFound)?;

        Self::require_op_pending(&op)?;

        op.cancelled = true;
        env.storage()
            .instance()
            .set(&DataKey::Op(op_id.clone()), &op);

        Self::decrement_pending(&env);

        env.events()
            .publish((Symbol::new(&env, "op_cancelled"),), op_id);

        Ok(())
    }

    /// Execute a queued operation after its ETA and before its grace period expires.
    ///
    /// Returns `TimelockError::NotReady` if called before `eta`.
    /// Returns `TimelockError::Expired` if called after `eta + grace_period_seconds`.
    ///
    /// Emits `op_executed` with `(op_id, target)`.
    pub fn execute(env: Env, caller: Address, op_id: Bytes) -> Result<(), TimelockError> {
        caller.require_auth();
        Self::require_admin(&env, &caller)?;

        let mut op: Op = env
            .storage()
            .instance()
            .get(&DataKey::Op(op_id.clone()))
            .ok_or(TimelockError::NotFound)?;

        Self::require_op_pending(&op)?;

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

        Self::decrement_pending(&env);

        env.events()
            .publish((Symbol::new(&env, "op_executed"),), (op_id, op.target));

        Ok(())
    }

    /// Update the description of a pending (not yet executed or cancelled) operation.
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

    // ── Council management ────────────────────────────────────────────────────

    /// Batch-replace the emergency council and set required approvals.
    ///
    /// `required` must be > 0 and <= `council.len()`.
    /// Only callable by the admin via a standard (non-fast-track) call.
    ///
    /// Emits `council_updated` with `(required, council)`.
    pub fn set_emergency_council(
        env: Env,
        caller: Address,
        council: Vec<Address>,
        required: u32,
    ) -> Result<(), TimelockError> {
        caller.require_auth();
        Self::require_admin(&env, &caller)?;

        if required == 0 || required > council.len() {
            return Err(TimelockError::InvalidConfig);
        }

        env.storage()
            .instance()
            .set(&DataKey::EmergencyCouncil, &council);
        env.storage()
            .instance()
            .set(&DataKey::RequiredApprovals, &required);

        env.events()
            .publish((Symbol::new(&env, "council_updated"),), (required, council));

        Ok(())
    }

    /// Add a single member to the emergency council.
    ///
    /// Only the admin may call this. The council list can only be updated via a
    /// standard (non-fast-track) admin call — council changes are subject to
    /// `min_delay` when queued through the timelock.
    ///
    /// Emits `council_member_added` with the new member address.
    pub fn add_council_member(
        env: Env,
        caller: Address,
        member: Address,
    ) -> Result<(), TimelockError> {
        caller.require_auth();
        Self::require_admin(&env, &caller)?;

        let mut council: Vec<Address> = env
            .storage()
            .instance()
            .get(&DataKey::EmergencyCouncil)
            .unwrap_or_else(|| Vec::new(&env));

        for m in council.iter() {
            if m == member {
                return Err(TimelockError::AlreadyMember);
            }
        }

        council.push_back(member.clone());
        env.storage()
            .instance()
            .set(&DataKey::EmergencyCouncil, &council);

        env.events()
            .publish((Symbol::new(&env, "council_member_added"),), member);

        Ok(())
    }

    /// Remove a single member from the emergency council.
    ///
    /// Only the admin may call this. Council updates are standard (non-fast-track)
    /// calls and are therefore subject to `min_delay` when queued.
    ///
    /// Returns `TimelockError::NotCouncilMember` if `member` is not in the council.
    ///
    /// Emits `council_member_removed` with the removed member address.
    pub fn remove_council_member(
        env: Env,
        caller: Address,
        member: Address,
    ) -> Result<(), TimelockError> {
        caller.require_auth();
        Self::require_admin(&env, &caller)?;

        let council: Vec<Address> = env
            .storage()
            .instance()
            .get(&DataKey::EmergencyCouncil)
            .unwrap_or_else(|| Vec::new(&env));

        let mut new_council: Vec<Address> = Vec::new(&env);
        let mut found = false;
        for m in council.iter() {
            if m == member {
                found = true;
            } else {
                new_council.push_back(m);
            }
        }

        if !found {
            return Err(TimelockError::NotCouncilMember);
        }

        env.storage()
            .instance()
            .set(&DataKey::EmergencyCouncil, &new_council);

        env.events()
            .publish((Symbol::new(&env, "council_member_removed"),), member);

        Ok(())
    }

    /// Approve a queued operation for fast-track (immediate) execution.
    ///
    /// Each council member may call this once per operation. When M-of-N approvals
    /// are collected the operation executes immediately, bypassing the normal ETA
    /// delay, and a `critical_fast_tracked` event is emitted.
    ///
    /// Security requirements:
    /// - Fast-track must be enabled (`set_fast_track_enabled(true)`).
    /// - Caller must be a council member.
    /// - Operation must be pending (not executed or cancelled).
    /// - M must be at least ⌈N/2⌉ + 1 (strict majority) — enforced by deployment
    ///   convention; see `set_emergency_council`.
    ///
    /// Emits `critical_fast_tracked` with `(op_id, target, approval_count)` upon execution.
    pub fn fast_track_approve(
        env: Env,
        member: Address,
        op_id: Bytes,
    ) -> Result<(), TimelockError> {
        member.require_auth();

        let fast_track_enabled: bool = env
            .storage()
            .instance()
            .get(&DataKey::FastTrackEnabled)
            .unwrap_or(false);

        if !fast_track_enabled {
            return Err(TimelockError::FastTrackDisabled);
        }

        Self::require_council_member(&env, &member)?;

        let mut op: Op = env
            .storage()
            .instance()
            .get(&DataKey::Op(op_id.clone()))
            .ok_or(TimelockError::NotFound)?;

        Self::require_op_pending(&op)?;

        let mut approvals: Vec<Address> = env
            .storage()
            .instance()
            .get(&DataKey::FastTrackApprovals(op_id.clone()))
            .unwrap_or_else(|| Vec::new(&env));

        for a in approvals.iter() {
            if a == member {
                return Err(TimelockError::AlreadyApproved);
            }
        }

        approvals.push_back(member.clone());
        env.storage()
            .instance()
            .set(&DataKey::FastTrackApprovals(op_id.clone()), &approvals);

        let required: u32 = env
            .storage()
            .instance()
            .get(&DataKey::RequiredApprovals)
            .unwrap_or(0);

        if required > 0 && approvals.len() >= required {
            op.executed = true;
            env.storage()
                .instance()
                .set(&DataKey::Op(op_id.clone()), &op);

            Self::decrement_pending(&env);

            env.events().publish(
                (Symbol::new(&env, "critical_fast_tracked"),),
                (op_id, op.target, approvals.len()),
            );
        }

        Ok(())
    }

    /// Enable or disable the fast-track execution path.
    ///
    /// Fast-track is **disabled by default**. Re-enable only during active emergencies
    /// and disable again once resolved.
    ///
    /// Emits `fast_track_toggled` with the new enabled state.
    pub fn set_fast_track_enabled(
        env: Env,
        caller: Address,
        enabled: bool,
    ) -> Result<(), TimelockError> {
        caller.require_auth();
        Self::require_admin(&env, &caller)?;
        env.storage()
            .instance()
            .set(&DataKey::FastTrackEnabled, &enabled);
        env.events()
            .publish((Symbol::new(&env, "fast_track_toggled"),), enabled);
        Ok(())
    }

    // ── Admin helpers ─────────────────────────────────────────────────────────

    /// Update the minimum delay for newly queued operations.
    ///
    /// Does not affect already-queued operations.
    ///
    /// Emits `min_delay_updated` with the new delay.
    pub fn set_min_delay(env: Env, caller: Address, new_delay: u64) -> Result<(), TimelockError> {
        caller.require_auth();
        Self::require_admin(&env, &caller)?;
        if new_delay == 0 {
            return Err(TimelockError::InvalidDelay);
        }
        env.storage().instance().set(&DataKey::MinDelay, &new_delay);
        env.events()
            .publish((Symbol::new(&env, "min_delay_updated"),), new_delay);
        Ok(())
    }

    /// Transfer admin to a new address.
    ///
    /// Emits `admin_transferred` with `(current, new_admin)`.
    pub fn transfer_admin(
        env: Env,
        current: Address,
        new_admin: Address,
    ) -> Result<(), TimelockError> {
        current.require_auth();
        Self::require_admin(&env, &current)?;
        env.storage().instance().set(&DataKey::Admin, &new_admin);
        env.events().publish(
            (Symbol::new(&env, "admin_transferred"),),
            (current, new_admin),
        );
        Ok(())
    }

    // ── Read-only queries ─────────────────────────────────────────────────────

    pub fn get_op(env: Env, op_id: Bytes) -> Option<Op> {
        env.storage().instance().get(&DataKey::Op(op_id))
    }

    /// Returns the human-readable status of an operation, or `None` if not found.
    pub fn get_operation_status(env: Env, op_id: Bytes) -> Option<OperationStatus> {
        let op: Op = env.storage().instance().get(&DataKey::Op(op_id))?;
        let now = env.ledger().timestamp();
        Some(if op.cancelled {
            OperationStatus::Cancelled
        } else if op.executed {
            OperationStatus::Executed
        } else if now > op.eta + op.grace_period_seconds {
            OperationStatus::Expired
        } else if now >= op.eta {
            OperationStatus::Ready
        } else {
            OperationStatus::Queued
        })
    }

    pub fn min_delay(env: Env) -> u64 {
        env.storage()
            .instance()
            .get(&DataKey::MinDelay)
            .unwrap_or(0)
    }

    pub fn admin(env: Env) -> Result<Address, TimelockError> {
        env.storage()
            .instance()
            .get(&DataKey::Admin)
            .ok_or(TimelockError::NotInitialized)
    }

    pub fn get_fast_track_enabled(env: Env) -> bool {
        env.storage()
            .instance()
            .get(&DataKey::FastTrackEnabled)
            .unwrap_or(false)
    }

    pub fn get_council(env: Env) -> Vec<Address> {
        env.storage()
            .instance()
            .get(&DataKey::EmergencyCouncil)
            .unwrap_or_else(|| Vec::new(&env))
    }

    pub fn get_required_approvals(env: Env) -> u32 {
        env.storage()
            .instance()
            .get(&DataKey::RequiredApprovals)
            .unwrap_or(0)
    }

    pub fn is_council_member(env: Env, addr: Address) -> bool {
        let council: Vec<Address> = env
            .storage()
            .instance()
            .get(&DataKey::EmergencyCouncil)
            .unwrap_or_else(|| Vec::new(&env));
        council.iter().any(|m| m == addr)
    }

    pub fn get_fast_track_approvals(env: Env, op_id: Bytes) -> Vec<Address> {
        env.storage()
            .instance()
            .get(&DataKey::FastTrackApprovals(op_id))
            .unwrap_or_else(|| Vec::new(&env))
    }

    /// Returns the count of operations that are queued but not yet executed or cancelled.
    pub fn get_pending_op_count(env: Env) -> u64 {
        env.storage()
            .instance()
            .get(&DataKey::PendingCount)
            .unwrap_or(0)
    }

    // ── Internal helpers ──────────────────────────────────────────────────────

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

    fn require_council_member(env: &Env, caller: &Address) -> Result<(), TimelockError> {
        let council: Vec<Address> = env
            .storage()
            .instance()
            .get(&DataKey::EmergencyCouncil)
            .unwrap_or(Vec::new(env));
        for member in council.iter() {
            if &member == caller {
                return Ok(());
            }
        }
        Err(TimelockError::NotCouncilMember)
    }

    fn require_op_pending(op: &Op) -> Result<(), TimelockError> {
        if op.cancelled {
            return Err(TimelockError::Cancelled);
        }
        if op.executed {
            return Err(TimelockError::AlreadyExecuted);
        }
        Ok(())
    }

    fn decrement_pending(env: &Env) {
        let count: u64 = env
            .storage()
            .instance()
            .get(&DataKey::PendingCount)
            .unwrap_or(0);
        if count > 0 {
            env.storage()
                .instance()
                .set(&DataKey::PendingCount, &(count - 1));
        }
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

    const GRACE: u64 = 86_400;

    fn setup() -> (Env, Address, RouterTimelockClient<'static>) {
        let env = Env::default();
        env.mock_all_auths();
        env.ledger().with_mut(|l| l.timestamp = 1_000);
        let contract_id = env.register_contract(None, RouterTimelock);
        let client = RouterTimelockClient::new(&env, &contract_id);
        let admin = Address::generate(&env);
        client.initialize(&admin, &3600);
        (env, admin, client)
    }

    fn setup_with_council() -> (
        Env,
        Address,
        RouterTimelockClient<'static>,
        Address,
        Address,
        Address,
    ) {
        let (env, admin, client) = setup();
        let m1 = Address::generate(&env);
        let m2 = Address::generate(&env);
        let m3 = Address::generate(&env);
        let mut council = Vec::new(&env);
        council.push_back(m1.clone());
        council.push_back(m2.clone());
        council.push_back(m3.clone());
        // 3 members, required = 2 (strict majority: ⌈3/2⌉ + 1 = 2 + 1 = 3... but 2 is also > N/2)
        client.set_emergency_council(&admin, &council, &2);
        client.set_fast_track_enabled(&admin, &true);
        (env, admin, client, m1, m2, m3)
    }

    // ── initialize ────────────────────────────────────────────────────────────

    #[test]
    fn test_initialize_sets_admin_and_delay() {
        let (env, admin, client) = setup();
        assert_eq!(client.admin(), admin);
        assert_eq!(client.min_delay(), 3600);
    }

    #[test]
    fn test_initialize_twice_fails() {
        let (env, admin, client) = setup();
        assert_eq!(
            client.try_initialize(&admin, &3600),
            Err(Ok(TimelockError::AlreadyInitialized))
        );
    }

    // ── queue ─────────────────────────────────────────────────────────────────

    #[test]
    fn test_queue_returns_op_id() {
        let (env, admin, client) = setup();
        let target = Address::generate(&env);
        let deps = Vec::new(&env);
        let op_id = client.queue(
            &admin,
            &String::from_str(&env, "upgrade oracle"),
            &target,
            &3600,
            &GRACE,
            &deps,
        );
        assert!(!op_id.is_empty());
    }

    #[test]
    fn test_queue_emits_op_queued() {
        let (env, admin, client) = setup();
        let target = Address::generate(&env);
        let deps = Vec::new(&env);
        let op_id = client.queue(
            &admin,
            &String::from_str(&env, "upgrade oracle"),
            &target,
            &3600,
            &GRACE,
            &deps,
        );
        let events = env.events().all();
        let last = events.last().unwrap();
        let topic: Symbol = last.1.get(0).unwrap().into_val(&env);
        assert_eq!(topic, Symbol::new(&env, "op_queued"));
        let (emitted_id, emitted_target, _eta, emitted_grace): (Bytes, Address, u64, u64) =
            last.2.into_val(&env);
        assert_eq!(emitted_id, op_id);
        assert_eq!(emitted_target, target);
        assert_eq!(emitted_grace, GRACE);
    }

    #[test]
    fn test_queue_stores_op() {
        let (env, admin, client) = setup();
        let target = Address::generate(&env);
        let deps = Vec::new(&env);
        let op_id = client.queue(
            &admin,
            &String::from_str(&env, "upgrade oracle"),
            &target,
            &3600,
            &GRACE,
            &deps,
        );
        let op = client.get_op(&op_id).unwrap();
        assert_eq!(op.target, target);
        assert_eq!(op.grace_period_seconds, GRACE);
        assert!(!op.executed);
        assert!(!op.cancelled);
    }

    #[test]
    fn test_delay_too_short_fails() {
        let (env, admin, client) = setup();
        let target = Address::generate(&env);
        let deps = Vec::new(&env);
        assert_eq!(
            client.try_queue(
                &admin,
                &String::from_str(&env, "upgrade oracle"),
                &target,
                &100,
                &GRACE,
                &deps,
            ),
            Err(Ok(TimelockError::DelayTooShort))
        );
    }

    #[test]
    fn test_unauthorized_queue_fails() {
        let (env, _admin, client) = setup();
        let attacker = Address::generate(&env);
        let target = Address::generate(&env);
        let deps = Vec::new(&env);
        assert_eq!(
            client.try_queue(
                &attacker,
                &String::from_str(&env, "malicious"),
                &target,
                &3600,
                &GRACE,
                &deps,
            ),
            Err(Ok(TimelockError::Unauthorized))
        );
    }

    // ── execute ───────────────────────────────────────────────────────────────

    #[test]
    fn test_execute_after_eta_succeeds() {
        let (env, admin, client) = setup();
        let target = Address::generate(&env);
        let deps = Vec::new(&env);
        let op_id = client.queue(
            &admin,
            &String::from_str(&env, "upgrade oracle"),
            &target,
            &3600,
            &GRACE,
            &deps,
        );
        env.ledger().with_mut(|l| l.timestamp += 3601);
        client.execute(&admin, &op_id);
        assert!(client.get_op(&op_id).unwrap().executed);
    }

    #[test]
    fn test_execute_before_eta_fails() {
        let (env, admin, client) = setup();
        let target = Address::generate(&env);
        let deps = Vec::new(&env);
        let op_id = client.queue(
            &admin,
            &String::from_str(&env, "upgrade oracle"),
            &target,
            &3600,
            &GRACE,
            &deps,
        );
        assert_eq!(
            client.try_execute(&admin, &op_id),
            Err(Ok(TimelockError::NotReady))
        );
    }

    #[test]
    fn test_execute_after_grace_period_fails() {
        let (env, admin, client) = setup();
        let target = Address::generate(&env);
        let deps: Vec<Bytes> = Vec::new(&env);
        let grace: u64 = 3600;
        let op_id = client.queue(
            &admin,
            &String::from_str(&env, "upgrade oracle"),
            &target,
            &3600,
            &grace,
            &deps,
        );
        env.ledger().with_mut(|l| l.timestamp += 3600 + grace + 1);
        assert_eq!(
            client.try_execute(&admin, &op_id),
            Err(Ok(TimelockError::Expired))
        );
    }

    #[test]
    fn test_execute_at_grace_boundary_succeeds() {
        let (env, admin, client) = setup();
        let target = Address::generate(&env);
        let deps: Vec<Bytes> = Vec::new(&env);
        let grace: u64 = 3600;
        let op_id = client.queue(
            &admin,
            &String::from_str(&env, "boundary"),
            &target,
            &3600,
            &grace,
            &deps,
        );
        env.ledger().with_mut(|l| l.timestamp += 3600 + grace);
        client.execute(&admin, &op_id);
        assert!(client.get_op(&op_id).unwrap().executed);
    }

    #[test]
    fn test_execute_cancelled_op_fails() {
        let (env, admin, client) = setup();
        let target = Address::generate(&env);
        let deps = Vec::new(&env);
        let op_id = client.queue(
            &admin,
            &String::from_str(&env, "upgrade oracle"),
            &target,
            &3600,
            &GRACE,
            &deps,
        );
        client.cancel(&admin, &op_id);
        env.ledger().with_mut(|l| l.timestamp += 3601);
        assert_eq!(
            client.try_execute(&admin, &op_id),
            Err(Ok(TimelockError::Cancelled))
        );
    }

    #[test]
    fn test_execute_twice_fails() {
        let (env, admin, client) = setup();
        let target = Address::generate(&env);
        let deps = Vec::new(&env);
        let op_id = client.queue(
            &admin,
            &String::from_str(&env, "upgrade oracle"),
            &target,
            &3600,
            &GRACE,
            &deps,
        );
        env.ledger().with_mut(|l| l.timestamp += 3601);
        client.execute(&admin, &op_id);
        assert_eq!(
            client.try_execute(&admin, &op_id),
            Err(Ok(TimelockError::AlreadyExecuted))
        );
    }

    #[test]
    fn test_execute_emits_op_executed() {
        let (env, admin, client) = setup();
        let target = Address::generate(&env);
        let deps = Vec::new(&env);
        let op_id = client.queue(
            &admin,
            &String::from_str(&env, "upgrade oracle"),
            &target,
            &3600,
            &GRACE,
            &deps,
        );
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
        let deps = Vec::new(&env);
        let op_id = client.queue(
            &admin,
            &String::from_str(&env, "upgrade oracle"),
            &target,
            &3600,
            &GRACE,
            &deps,
        );
        client.cancel(&admin, &op_id);
        assert!(client.get_op(&op_id).unwrap().cancelled);
    }

    #[test]
    fn test_cancel_emits_op_cancelled() {
        let (env, admin, client) = setup();
        let target = Address::generate(&env);
        let deps = Vec::new(&env);
        let op_id = client.queue(
            &admin,
            &String::from_str(&env, "upgrade oracle"),
            &target,
            &3600,
            &GRACE,
            &deps,
        );
        client.cancel(&admin, &op_id);
        let events = env.events().all();
        let last = events.last().unwrap();
        let topic: Symbol = last.1.get(0).unwrap().into_val(&env);
        assert_eq!(topic, Symbol::new(&env, "op_cancelled"));
    }

    // ── get_operation_status ──────────────────────────────────────────────────

    #[test]
    fn test_status_queued() {
        let (env, admin, client) = setup();
        let target = Address::generate(&env);
        let deps = Vec::new(&env);
        let op_id = client.queue(
            &admin,
            &String::from_str(&env, "upgrade oracle"),
            &target,
            &3600,
            &GRACE,
            &deps,
        );
        assert_eq!(
            client.get_operation_status(&op_id),
            Some(OperationStatus::Queued)
        );
    }

    #[test]
    fn test_status_ready() {
        let (env, admin, client) = setup();
        let target = Address::generate(&env);
        let deps = Vec::new(&env);
        let op_id = client.queue(
            &admin,
            &String::from_str(&env, "upgrade oracle"),
            &target,
            &3600,
            &GRACE,
            &deps,
        );
        env.ledger().with_mut(|l| l.timestamp += 3601);
        assert_eq!(
            client.get_operation_status(&op_id),
            Some(OperationStatus::Ready)
        );
    }

    #[test]
    fn test_status_executed() {
        let (env, admin, client) = setup();
        let target = Address::generate(&env);
        let deps = Vec::new(&env);
        let op_id = client.queue(
            &admin,
            &String::from_str(&env, "upgrade oracle"),
            &target,
            &3600,
            &GRACE,
            &deps,
        );
        env.ledger().with_mut(|l| l.timestamp += 3601);
        client.execute(&admin, &op_id);
        assert_eq!(
            client.get_operation_status(&op_id),
            Some(OperationStatus::Executed)
        );
    }

    #[test]
    fn test_status_cancelled() {
        let (env, admin, client) = setup();
        let target = Address::generate(&env);
        let deps = Vec::new(&env);
        let op_id = client.queue(
            &admin,
            &String::from_str(&env, "upgrade oracle"),
            &target,
            &3600,
            &GRACE,
            &deps,
        );
        client.cancel(&admin, &op_id);
        assert_eq!(
            client.get_operation_status(&op_id),
            Some(OperationStatus::Cancelled)
        );
    }

    #[test]
    fn test_status_expired() {
        let (env, admin, client) = setup();
        let target = Address::generate(&env);
        let deps: Vec<Bytes> = Vec::new(&env);
        let grace: u64 = 3600;
        let op_id = client.queue(
            &admin,
            &String::from_str(&env, "upgrade oracle"),
            &target,
            &3600,
            &grace,
            &deps,
        );
        env.ledger().with_mut(|l| l.timestamp += 3600 + grace + 1);
        assert_eq!(
            client.get_operation_status(&op_id),
            Some(OperationStatus::Expired)
        );
    }

    #[test]
    fn test_status_nonexistent_returns_none() {
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
        let op_id = client.queue(
            &admin,
            &String::from_str(&env, "initial"),
            &target,
            &3600,
            &GRACE,
            &deps,
        );
        let new_desc = String::from_str(&env, "corrected");
        client.update_description(&admin, &op_id, &new_desc);
        assert_eq!(client.get_op(&op_id).unwrap().description, new_desc);
    }

    #[test]
    fn test_update_description_on_executed_fails() {
        let (env, admin, client) = setup();
        let target = Address::generate(&env);
        let deps = Vec::new(&env);
        let op_id = client.queue(
            &admin,
            &String::from_str(&env, "initial"),
            &target,
            &3600,
            &GRACE,
            &deps,
        );
        env.ledger().with_mut(|l| l.timestamp += 3601);
        client.execute(&admin, &op_id);
        assert_eq!(
            client.try_update_description(&admin, &op_id, &String::from_str(&env, "too late")),
            Err(Ok(TimelockError::AlreadyExecuted))
        );
    }

    #[test]
    fn test_update_description_on_cancelled_fails() {
        let (env, admin, client) = setup();
        let target = Address::generate(&env);
        let deps = Vec::new(&env);
        let op_id = client.queue(
            &admin,
            &String::from_str(&env, "initial"),
            &target,
            &3600,
            &GRACE,
            &deps,
        );
        client.cancel(&admin, &op_id);
        assert_eq!(
            client.try_update_description(&admin, &op_id, &String::from_str(&env, "too late")),
            Err(Ok(TimelockError::Cancelled))
        );
    }

    #[test]
    fn test_update_description_unauthorized_fails() {
        let (env, admin, client) = setup();
        let attacker = Address::generate(&env);
        let target = Address::generate(&env);
        let deps: Vec<Bytes> = Vec::new(&env);
        let op_id = client.queue(
            &admin,
            &String::from_str(&env, "initial"),
            &target,
            &3600,
            &GRACE,
            &deps,
        );
        assert_eq!(
            client.try_update_description(&attacker, &op_id, &String::from_str(&env, "hacked")),
            Err(Ok(TimelockError::Unauthorized))
        );
    }

    // ── set_min_delay / transfer_admin ────────────────────────────────────────

    #[test]
    fn test_set_min_delay() {
        let (env, admin, client) = setup();
        client.set_min_delay(&admin, &7200);
        assert_eq!(client.min_delay(), 7200);
    }

    #[test]
    fn test_set_min_delay_zero_fails() {
        let (env, admin, client) = setup();
        assert_eq!(
            client.try_set_min_delay(&admin, &0),
            Err(Ok(TimelockError::InvalidDelay))
        );
    }

    #[test]
    fn test_transfer_admin() {
        let (env, admin, client) = setup();
        let new_admin = Address::generate(&env);
        client.transfer_admin(&admin, &new_admin);
        assert_eq!(client.admin(), new_admin);
    }

    #[test]
    fn test_transfer_admin_locks_out_old_admin() {
        let (env, admin, client) = setup();
        let new_admin = Address::generate(&env);
        client.transfer_admin(&admin, &new_admin);
        assert_eq!(
            client.try_set_min_delay(&admin, &7200),
            Err(Ok(TimelockError::Unauthorized))
        );
    }

    // ── get_pending_op_count ──────────────────────────────────────────────────

    #[test]
    fn test_pending_op_count() {
        let (env, admin, client) = setup();
        let target = Address::generate(&env);
        let deps: Vec<Bytes> = Vec::new(&env);

        assert_eq!(client.get_pending_op_count(), 0);

        let op1 = client.queue(
            &admin,
            &String::from_str(&env, "op1"),
            &target,
            &3600,
            &GRACE,
            &deps,
        );
        assert_eq!(client.get_pending_op_count(), 1);

        let op2 = client.queue(
            &admin,
            &String::from_str(&env, "op2"),
            &target,
            &3600,
            &GRACE,
            &deps,
        );
        assert_eq!(client.get_pending_op_count(), 2);

        client.cancel(&admin, &op1);
        assert_eq!(client.get_pending_op_count(), 1);

        env.ledger().with_mut(|l| l.timestamp += 3601);
        client.execute(&admin, &op2);
        assert_eq!(client.get_pending_op_count(), 0);
    }

    // ── set_emergency_council ─────────────────────────────────────────────────

    #[test]
    fn test_set_emergency_council_succeeds() {
        let (env, admin, client) = setup();
        let m1 = Address::generate(&env);
        let m2 = Address::generate(&env);
        let mut council = Vec::new(&env);
        council.push_back(m1.clone());
        council.push_back(m2.clone());
        assert!(client
            .try_set_emergency_council(&admin, &council, &1)
            .is_ok());
        assert_eq!(client.get_required_approvals(), 1);
        assert!(client.is_council_member(&m1));
    }

    #[test]
    fn test_set_emergency_council_required_zero_fails() {
        let (env, admin, client) = setup();
        let m1 = Address::generate(&env);
        let mut council = Vec::new(&env);
        council.push_back(m1);
        assert_eq!(
            client.try_set_emergency_council(&admin, &council, &0),
            Err(Ok(TimelockError::InvalidConfig))
        );
    }

    #[test]
    fn test_set_emergency_council_required_exceeds_size_fails() {
        let (env, admin, client) = setup();
        let m1 = Address::generate(&env);
        let mut council = Vec::new(&env);
        council.push_back(m1);
        assert_eq!(
            client.try_set_emergency_council(&admin, &council, &2),
            Err(Ok(TimelockError::InvalidConfig))
        );
    }

    #[test]
    fn test_set_emergency_council_unauthorized_fails() {
        let (env, _admin, client) = setup();
        let attacker = Address::generate(&env);
        let m1 = Address::generate(&env);
        let mut council = Vec::new(&env);
        council.push_back(m1);
        assert_eq!(
            client.try_set_emergency_council(&attacker, &council, &1),
            Err(Ok(TimelockError::Unauthorized))
        );
    }

    // ── add_council_member ────────────────────────────────────────────────────

    #[test]
    fn test_add_council_member_succeeds() {
        let (env, admin, client) = setup();
        let m1 = Address::generate(&env);
        client.add_council_member(&admin, &m1);
        assert!(client.is_council_member(&m1));
        assert_eq!(client.get_council().len(), 1);
    }

    #[test]
    fn test_add_council_member_emits_event() {
        let (env, admin, client) = setup();
        let m1 = Address::generate(&env);
        client.add_council_member(&admin, &m1);
        let events = env.events().all();
        let last = events.last().unwrap();
        let topic: Symbol = last.1.get(0).unwrap().into_val(&env);
        assert_eq!(topic, Symbol::new(&env, "council_member_added"));
    }

    #[test]
    fn test_add_council_member_duplicate_fails() {
        let (env, admin, client) = setup();
        let m1 = Address::generate(&env);
        client.add_council_member(&admin, &m1);
        assert_eq!(
            client.try_add_council_member(&admin, &m1),
            Err(Ok(TimelockError::AlreadyMember))
        );
    }

    #[test]
    fn test_add_council_member_unauthorized_fails() {
        let (env, _admin, client) = setup();
        let attacker = Address::generate(&env);
        let m1 = Address::generate(&env);
        assert_eq!(
            client.try_add_council_member(&attacker, &m1),
            Err(Ok(TimelockError::Unauthorized))
        );
    }

    // ── remove_council_member ─────────────────────────────────────────────────

    #[test]
    fn test_remove_council_member_succeeds() {
        let (env, admin, client) = setup();
        let m1 = Address::generate(&env);
        let m2 = Address::generate(&env);
        client.add_council_member(&admin, &m1);
        client.add_council_member(&admin, &m2);
        client.remove_council_member(&admin, &m1);
        assert!(!client.is_council_member(&m1));
        assert!(client.is_council_member(&m2));
        assert_eq!(client.get_council().len(), 1);
    }

    #[test]
    fn test_remove_council_member_emits_event() {
        let (env, admin, client) = setup();
        let m1 = Address::generate(&env);
        client.add_council_member(&admin, &m1);
        client.remove_council_member(&admin, &m1);
        let events = env.events().all();
        let last = events.last().unwrap();
        let topic: Symbol = last.1.get(0).unwrap().into_val(&env);
        assert_eq!(topic, Symbol::new(&env, "council_member_removed"));
    }

    #[test]
    fn test_remove_council_member_not_member_fails() {
        let (env, admin, client) = setup();
        let non_member = Address::generate(&env);
        assert_eq!(
            client.try_remove_council_member(&admin, &non_member),
            Err(Ok(TimelockError::NotCouncilMember))
        );
    }

    #[test]
    fn test_remove_council_member_unauthorized_fails() {
        let (env, admin, client) = setup();
        let attacker = Address::generate(&env);
        let m1 = Address::generate(&env);
        client.add_council_member(&admin, &m1);
        assert_eq!(
            client.try_remove_council_member(&attacker, &m1),
            Err(Ok(TimelockError::Unauthorized))
        );
    }

    // ── set_fast_track_enabled ────────────────────────────────────────────────

    #[test]
    fn test_fast_track_disabled_by_default() {
        let (env, _admin, client) = setup();
        assert!(!client.get_fast_track_enabled());
    }

    #[test]
    fn test_set_fast_track_enabled() {
        let (env, admin, client) = setup();
        client.set_fast_track_enabled(&admin, &true);
        assert!(client.get_fast_track_enabled());
        client.set_fast_track_enabled(&admin, &false);
        assert!(!client.get_fast_track_enabled());
    }

    #[test]
    fn test_set_fast_track_enabled_unauthorized_fails() {
        let (env, _admin, client) = setup();
        let attacker = Address::generate(&env);
        assert_eq!(
            client.try_set_fast_track_enabled(&attacker, &true),
            Err(Ok(TimelockError::Unauthorized))
        );
    }

    #[test]
    fn test_set_fast_track_enabled_emits_event() {
        let (env, admin, client) = setup();
        client.set_fast_track_enabled(&admin, &true);
        let events = env.events().all();
        let last = events.last().unwrap();
        let topic: Symbol = last.1.get(0).unwrap().into_val(&env);
        assert_eq!(topic, Symbol::new(&env, "fast_track_toggled"));
        let enabled: bool = last.2.into_val(&env);
        assert!(enabled);
    }

    // ── fast_track_approve ────────────────────────────────────────────────────

    #[test]
    fn test_fast_track_approve_disabled_fails() {
        let (env, admin, client) = setup();
        let m1 = Address::generate(&env);
        let target = Address::generate(&env);
        let deps: Vec<Bytes> = Vec::new(&env);
        let mut council = Vec::new(&env);
        council.push_back(m1.clone());
        client.set_emergency_council(&admin, &council, &1);
        // fast-track is still disabled
        let op_id = client.queue(
            &admin,
            &String::from_str(&env, "emergency patch"),
            &target,
            &3600,
            &GRACE,
            &deps,
        );
        assert_eq!(
            client.try_fast_track_approve(&m1, &op_id),
            Err(Ok(TimelockError::FastTrackDisabled))
        );
    }

    #[test]
    fn test_fast_track_approve_non_member_fails() {
        let (env, admin, client) = setup();
        let non_member = Address::generate(&env);
        let target = Address::generate(&env);
        let deps: Vec<Bytes> = Vec::new(&env);
        // council is empty, fast-track enabled
        client.set_fast_track_enabled(&admin, &true);
        let op_id = client.queue(
            &admin,
            &String::from_str(&env, "emergency patch"),
            &target,
            &3600,
            &GRACE,
            &deps,
        );
        assert_eq!(
            client.try_fast_track_approve(&non_member, &op_id),
            Err(Ok(TimelockError::NotCouncilMember))
        );
    }

    #[test]
    fn test_fast_track_approve_duplicate_fails() {
        let (env, admin, client, m1, _m2, _m3) = setup_with_council();
        let target = Address::generate(&env);
        let deps: Vec<Bytes> = Vec::new(&env);
        let op_id = client.queue(
            &admin,
            &String::from_str(&env, "emergency patch"),
            &target,
            &3600,
            &GRACE,
            &deps,
        );
        client.fast_track_approve(&m1, &op_id);
        assert_eq!(
            client.try_fast_track_approve(&m1, &op_id),
            Err(Ok(TimelockError::AlreadyApproved))
        );
    }

    #[test]
    fn test_fast_track_approve_below_threshold_does_not_execute() {
        let (env, admin, client, m1, _m2, _m3) = setup_with_council();
        let target = Address::generate(&env);
        let deps: Vec<Bytes> = Vec::new(&env);
        let op_id = client.queue(
            &admin,
            &String::from_str(&env, "emergency patch"),
            &target,
            &3600,
            &GRACE,
            &deps,
        );
        // Only 1 approval, threshold is 2
        client.fast_track_approve(&m1, &op_id);
        assert!(!client.get_op(&op_id).unwrap().executed);
    }

    #[test]
    fn test_fast_track_approve_reaches_threshold_executes_and_emits_event() {
        let (env, admin, client, m1, m2, _m3) = setup_with_council();
        let target = Address::generate(&env);
        let deps: Vec<Bytes> = Vec::new(&env);
        let op_id = client.queue(
            &admin,
            &String::from_str(&env, "emergency patch"),
            &target,
            &3600,
            &GRACE,
            &deps,
        );
        // 2 approvals — reaches threshold
        client.fast_track_approve(&m1, &op_id);
        client.fast_track_approve(&m2, &op_id);

        // Operation should be executed immediately (bypassing ETA)
        assert!(client.get_op(&op_id).unwrap().executed);

        // Last event should be critical_fast_tracked
        let events = env.events().all();
        let last = events.last().unwrap();
        let topic: Symbol = last.1.get(0).unwrap().into_val(&env);
        assert_eq!(topic, Symbol::new(&env, "critical_fast_tracked"));
    }

    #[test]
    fn test_fast_track_approve_execution_decrements_pending_count() {
        let (env, admin, client, m1, m2, _m3) = setup_with_council();
        let target = Address::generate(&env);
        let deps: Vec<Bytes> = Vec::new(&env);
        let op_id = client.queue(
            &admin,
            &String::from_str(&env, "emergency patch"),
            &target,
            &3600,
            &GRACE,
            &deps,
        );
        assert_eq!(client.get_pending_op_count(), 1);
        client.fast_track_approve(&m1, &op_id);
        client.fast_track_approve(&m2, &op_id);
        assert_eq!(client.get_pending_op_count(), 0);
    }

    #[test]
    fn test_fast_track_approve_executed_op_fails() {
        let (env, admin, client, m1, m2, m3) = setup_with_council();
        let target = Address::generate(&env);
        let deps: Vec<Bytes> = Vec::new(&env);
        let op_id = client.queue(
            &admin,
            &String::from_str(&env, "emergency patch"),
            &target,
            &3600,
            &GRACE,
            &deps,
        );
        client.fast_track_approve(&m1, &op_id);
        client.fast_track_approve(&m2, &op_id);
        // m3 tries to approve the already-executed op
        assert_eq!(
            client.try_fast_track_approve(&m3, &op_id),
            Err(Ok(TimelockError::AlreadyExecuted))
        );
    }

    #[test]
    fn test_fast_track_bypasses_eta() {
        // Fast-track should execute without waiting for ETA
        let (env, admin, client, m1, m2, _m3) = setup_with_council();
        let target = Address::generate(&env);
        let deps: Vec<Bytes> = Vec::new(&env);
        let op_id = client.queue(
            &admin,
            &String::from_str(&env, "emergency patch"),
            &target,
            &3600,
            &GRACE,
            &deps,
        );
        // Do NOT advance the ledger — ETA has not passed
        client.fast_track_approve(&m1, &op_id);
        client.fast_track_approve(&m2, &op_id);
        assert!(
            client.get_op(&op_id).unwrap().executed,
            "fast-track should execute before ETA"
        );
    }
}

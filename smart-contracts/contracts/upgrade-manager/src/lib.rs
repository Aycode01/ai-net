#![no_std]

//! # Upgrade Manager Contract
//!
//! Timelock-governed, multi-signature upgrade management for Soroban contracts.
//!
//! ## Scope: this is a *self*-upgrade manager
//!
//! Soroban's only upgrade primitive is
//! `env.deployer().update_current_contract_wasm(hash)`. Per the SDK docs
//! (soroban-sdk 22.0.11, `src/deploy.rs`):
//!
//! > Replaces the executable of **the current contract** with the provided Wasm.
//! > ... The function won't do anything immediately. The contract executable
//! > will only be updated after the invocation has successfully finished.
//!
//! There is no target-address parameter, so **a contract can only ever upgrade
//! itself**. This contract therefore governs its own WASM. Other contracts that
//! want the same governance copy the pattern: they depend on this crate as a
//! library (`default-features = false`) and implement the [`Upgradeable`] trait,
//! as `agent_registry` does. This crate is the reference implementation and the
//! shared source of types and constants — not a cross-contract upgrade authority.
//!
//! ## Lifecycle
//!
//! ```text
//! Proposed ──approve*──> Approved ──threshold reached──> PendingTimelock
//!     │                                                    │
//!     │                                          eta elapsed │
//!     │                                                    ▼
//!     └────────────────── insufficient ────────────────> Ready
//!                                                          │ execute
//!                                                          ▼
//!                                          Executed ──rollback──> RolledBack
//!     any non-terminal ──now > expires_at──> Expired ──sweep──> (reaped)
//! ```
//!
//! ## Timelock: deliberately stricter than `agent_registry`
//!
//! `agent_registry` sets `eta = created_at + timelock_delay` **at proposal
//! time**. That is flawed: if the signer set is widened, a proposal can sit
//! unapproved past its `eta` and become *instantly* executable the moment the
//! last approval lands, so the timelock provides no real delay.
//!
//! This contract instead starts the clock **only when the approval threshold is
//! first reached** (`eta` is `0` until then). The waiting period is therefore
//! always observed in full, after the decision to upgrade is final. This is an
//! intentional deviation from the sibling contract; see `approve_upgrade`.
//!
//! ## Rollback is a *code* revert, not a state revert
//!
//! `rollback_upgrade` re-invokes `update_current_contract_wasm` with the
//! previous hash — the same primitive as an upgrade, which is the only thing
//! the platform offers. It does **not** undo any storage mutation performed by
//! `execute_post_upgrade_migration`. Today every migration function in
//! `migration.rs` is a stub that writes nothing, so no data is at risk; but a
//! future destructive migration must not rely on rollback to undo itself. See
//! `migration::is_migration_reversible`.
//!
//! ## Security model
//!
//! - Only governance members (single admin, or the multisig signer set when one
//!   is configured) may propose, approve, execute or roll back.
//! - `require_auth()` is called on the *caller-supplied* address, so Soroban
//!   enforces the signature — membership alone is not enough.
//! - Execution requires BOTH the timelock to have elapsed AND the approval
//!   threshold to be met.
//! - A single-signature emergency path (`set_admin`, `set_multisig_config`,
//!   `pause`) is deliberately retained even when a multisig is configured, so a
//!   misconfigured signer set can always be repaired.

pub mod events;
mod migration;
pub mod strutil;
pub mod upgradeable;

use events::*;
use migration::*;
use soroban_sdk::{
    contract, contracterror, contractimpl, contracttype, symbol_short, Address, BytesN, Env,
    String, Vec,
};
pub use upgradeable::*;

// ─── Constants ───────────────────────────────────────────────────────────────

/// Rollback window in ledgers (48h at ~5s per ledger).
///
/// UNIT NOTE: this is a *ledger-sequence* window, whereas [`DEFAULT_TIMELOCK_DELAY`]
/// and [`DEFAULT_PROPOSAL_EXPIRY`] are *wall-clock seconds*. The inconsistency
/// is preserved deliberately — `agent_registry::get_upgrade_status` derives its
/// own rollback deadline from this constant, so changing the unit would silently
/// break that contract. Documented rather than fixed.
pub const ROLLBACK_WINDOW_LEDGERS: u32 = 34_560;

/// Default timelock delay in seconds (24h).
pub const DEFAULT_TIMELOCK_DELAY: u64 = 86_400;

/// Default proposal validity period in seconds (7 days).
pub const DEFAULT_PROPOSAL_EXPIRY: u64 = 604_800;

/// Approval threshold applied when no multisig is configured: the proposer alone.
pub const DEFAULT_THRESHOLD: u32 = 1;

/// Maximum proposals examined by a single sweep call, to bound gas.
pub const MAX_SWEEP_PROPOSALS: u32 = 50;

/// Default TTL threshold for storage extension
pub const TTL_THRESHOLD: u32 = 100_000;
/// Target TTL after extension (~31 days)
pub const TTL_EXTEND_TO: u32 = 535_680;

/// Gas budget constants for upgrade operations
pub const GAS_UPGRADE_BASE: u64 = 500_000;
pub const GAS_MIGRATION_PER_ITEM: u64 = 10_000;
pub const GAS_ROLLBACK_BASE: u64 = 200_000;

// ─── Types ───────────────────────────────────────────────────────────────────

/// Contract version information
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ContractVersion {
    pub version: String,
    pub wasm_hash: BytesN<32>,
    pub upgrade_ledger: u32,
    pub description: String,
    pub admin: Address,
    pub rollback_deadline: u32,
}

/// Migration execution plan
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MigrationPlan {
    pub pre_migration_checks: Vec<String>,
    pub data_transformations: Vec<String>,
    pub post_migration_validations: Vec<String>,
    pub estimated_items: u32,
}

/// Rollback record for tracking rollback eligibility
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RollbackRecord {
    pub previous_version: ContractVersion,
    pub rollback_deadline: u32,
    pub can_rollback: bool,
}

/// Multi-signature administration configuration.
///
/// Mirrors `agent_registry::MultisigConfig` so the two contracts stay
/// idiomatic with each other.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MultisigConfig {
    /// Authorized governance members.
    pub admins: Vec<Address>,
    /// Required approval count (M of N).
    pub threshold: u32,
    /// Delay in seconds between reaching threshold and becoming executable.
    pub timelock_delay: u64,
}

/// Lifecycle state of an upgrade proposal.
///
/// `Ready` is never persisted: it is *derived* at read time from
/// `PendingTimelock + now >= eta`, because no transaction runs at the moment
/// the timelock elapses.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ProposalStatus {
    Proposed = 0,
    Approved = 1,
    PendingTimelock = 2,
    Ready = 3,
    Executed = 4,
    RolledBack = 5,
    Expired = 6,
}

impl ProposalStatus {
    /// Terminal states can never transition again.
    pub fn is_terminal(&self) -> bool {
        matches!(
            self,
            ProposalStatus::Executed | ProposalStatus::RolledBack | ProposalStatus::Expired
        )
    }
}

/// An upgrade proposal awaiting approval, timelock, and execution.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UpgradeProposal {
    pub id: u64,
    pub proposer: Address,
    pub new_version: String,
    pub new_wasm_hash: BytesN<32>,
    pub description: String,
    pub migration_plan: MigrationPlan,
    /// Ledger timestamp (seconds) at proposal creation.
    pub created_at: u64,
    /// Earliest execution time. `0` until the approval threshold is reached.
    pub eta: u64,
    /// Timestamp after which this proposal can never be executed.
    pub expires_at: u64,
    /// Governance members who have approved. Length is the approval count.
    pub approvals: Vec<Address>,
    pub status: ProposalStatus,
    pub validated: bool,
    pub estimated_gas: u64,
    /// Captured at execution time, for rollback.
    pub previous_version: String,
    pub previous_wasm_hash: BytesN<32>,
    pub executed_ledger: u32,
    pub rollback_deadline: u32,
}

/// Storage keys for upgrade manager data.
///
/// STORAGE-LAYOUT NOTE (issue #488): `Proposal` and `Rollback` changed from
/// unit variants to tuple variants keyed by proposal id, and `MultisigConfig` /
/// `NextProposalId` were appended. New variants are appended at the end so
/// existing discriminants are not shifted. The two *changed* slots only ever
/// hold transient records (a proposal is superseded once executed, a rollback
/// record is deleted once consumed), so the blast radius of this change is
/// limited to an in-flight proposal or rollback record at upgrade time.
#[contracttype]
#[derive(Clone)]
pub enum DataKey {
    /// Current admin address (retained as the emergency single-signature path)
    Admin,
    /// Whether the contract is paused
    Paused,
    /// Current contract version
    CurrentVersion,
    /// Version history (version_string -> ContractVersion)
    Version(String),
    /// Upgrade proposal, keyed by proposal id
    Proposal(u64),
    /// Rollback record, keyed by proposal id
    Rollback(u64),
    /// Migration state during upgrade
    MigrationState,
    /// Contract-specific upgrade hooks
    UpgradeHooks,
    /// Multisig configuration, when configured
    MultisigConfig,
    /// Monotonic proposal id counter
    NextProposalId,
}

/// Upgrade operation errors.
///
/// Codes 1-13 are the original set and are frozen — never renumber them, as the
/// values are part of the contract's ABI. New variants start at 14.
#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq, PartialOrd, Ord)]
#[repr(u32)]
pub enum UpgradeError {
    /// Caller is not authorized to perform upgrade operations
    Unauthorized = 1,
    /// Version already exists or is invalid
    InvalidVersion = 2,
    /// No upgrade proposal exists
    NoProposal = 3,
    /// Upgrade proposal has not been validated
    ProposalNotValidated = 4,
    /// Pre-upgrade validation failed
    PreUpgradeValidationFailed = 5,
    /// Migration execution failed
    MigrationFailed = 6,
    /// Post-upgrade validation failed
    PostUpgradeValidationFailed = 7,
    /// Rollback deadline has passed
    RollbackDeadlineExpired = 8,
    /// No rollback available
    NoRollbackAvailable = 9,
    /// Contract not found or not upgradeable
    ContractNotUpgradeable = 10,
    /// Insufficient gas budget for migration
    InsufficientGasBudget = 11,
    /// Version downgrade not allowed without explicit rollback
    DowngradeNotAllowed = 12,
    /// The contract is paused and cannot accept mutations
    ContractPaused = 13,
    /// Signer has already approved this proposal
    AlreadyApproved = 14,
    /// Approval threshold not met
    InsufficientApprovals = 15,
    /// Timelock has not yet elapsed
    TimelockNotElapsed = 16,
    /// Proposal validity window has elapsed
    ProposalExpired = 17,
    /// Proposal has already been executed or rolled back
    ProposalAlreadyExecuted = 18,
    /// Multisig configuration is invalid
    InvalidMultisigConfig = 19,
    /// Proposal has not been validated yet
    ProposalNotValidatedForExecution = 20,
}

/// Main upgrade manager contract
#[contract]
pub struct UpgradeManager;

// ─── Helpers ─────────────────────────────────────────────────────────────────

fn extend_ttl_for_key(env: &Env, key: &DataKey) {
    if env.storage().persistent().has(key) {
        env.storage()
            .persistent()
            .extend_ttl(key, TTL_THRESHOLD, TTL_EXTEND_TO);
    }
}

fn require_admin(env: &Env) -> Result<Address, UpgradeError> {
    let admin: Address = env
        .storage()
        .instance()
        .get(&DataKey::Admin)
        .ok_or(UpgradeError::Unauthorized)?;
    admin.require_auth();
    Ok(admin)
}

fn require_not_paused(env: &Env) -> Result<(), UpgradeError> {
    let paused: bool = env
        .storage()
        .instance()
        .get(&DataKey::Paused)
        .unwrap_or(false);
    if paused {
        return Err(UpgradeError::ContractPaused);
    }
    Ok(())
}

fn get_current_version(env: &Env) -> Option<ContractVersion> {
    env.storage().persistent().get(&DataKey::CurrentVersion)
}

fn get_multisig(env: &Env) -> Option<MultisigConfig> {
    env.storage().instance().get(&DataKey::MultisigConfig)
}

fn next_proposal_id(env: &Env) -> u64 {
    env.storage()
        .instance()
        .get(&DataKey::NextProposalId)
        .unwrap_or(1)
}

fn load_proposal(env: &Env, id: u64) -> Result<UpgradeProposal, UpgradeError> {
    env.storage()
        .persistent()
        .get(&DataKey::Proposal(id))
        .ok_or(UpgradeError::NoProposal)
}

fn save_proposal(env: &Env, proposal: &UpgradeProposal) {
    env.storage()
        .persistent()
        .set(&DataKey::Proposal(proposal.id), proposal);
    extend_ttl_for_key(env, &DataKey::Proposal(proposal.id));
}

/// Effective threshold and timelock delay for the current configuration.
///
/// With no multisig configured this degrades to single-admin governance:
/// threshold 1 (the proposer's implicit approval) and the default 24h delay,
/// so a single-admin deployment still gets a real timelock.
fn effective_config(env: &Env) -> (u32, u64) {
    match get_multisig(env) {
        Some(cfg) => (cfg.threshold, cfg.timelock_delay),
        None => (DEFAULT_THRESHOLD, DEFAULT_TIMELOCK_DELAY),
    }
}

/// Whether `addr` may participate in upgrade governance.
fn is_governance_member(env: &Env, addr: &Address) -> bool {
    if let Some(admin) = env.storage().instance().get::<_, Address>(&DataKey::Admin) {
        if &admin == addr {
            return true;
        }
    }
    match get_multisig(env) {
        Some(cfg) => cfg.admins.contains(addr),
        None => false,
    }
}

/// Require that `member` signed the transaction and is a governance member.
///
/// `require_auth()` is what actually proves authorization; the membership check
/// only narrows who is allowed at all. Both are required.
fn require_governance_member(env: &Env, member: &Address) -> Result<(), UpgradeError> {
    member.require_auth();
    if !is_governance_member(env, member) {
        return Err(UpgradeError::Unauthorized);
    }
    Ok(())
}

/// The status a proposal *actually* has right now.
///
/// `Ready` is derived rather than stored, because no transaction runs at the
/// moment the timelock elapses.
fn effective_status(proposal: &UpgradeProposal, now: u64) -> ProposalStatus {
    if proposal.status.is_terminal() {
        return proposal.status.clone();
    }
    if now > proposal.expires_at {
        return ProposalStatus::Expired;
    }
    if proposal.status == ProposalStatus::PendingTimelock && proposal.eta > 0 && now >= proposal.eta
    {
        return ProposalStatus::Ready;
    }
    proposal.status.clone()
}

/// Returns `true` when `proposed` sorts strictly after `current`.
///
/// [`String`] implements `Ord` via the host's lexicographic byte comparison,
/// which works identically natively and under `wasm32v1-none`. Version tags are
/// therefore compared as byte strings, matching the ordering the registry has
/// always used. Callers needing semver precedence must zero-pad components.
fn is_version_newer(current: &String, proposed: &String) -> bool {
    proposed > current
}

// ─── Contract Implementation ─────────────────────────────────────────────────

#[cfg(feature = "contract")]
#[contractimpl]
impl UpgradeManager {
    /// Initialize the upgrade manager with an admin and initial version.
    pub fn initialize(
        env: Env,
        admin: Address,
        initial_version: String,
        initial_wasm_hash: BytesN<32>,
    ) -> Result<(), UpgradeError> {
        if env.storage().instance().has(&DataKey::Admin) {
            return Err(UpgradeError::InvalidVersion);
        }

        admin.require_auth();
        env.storage().instance().set(&DataKey::Admin, &admin);
        env.storage().instance().set(&DataKey::Paused, &false);
        env.storage()
            .instance()
            .set(&DataKey::NextProposalId, &1u64);

        let initial = ContractVersion {
            version: initial_version.clone(),
            wasm_hash: initial_wasm_hash,
            upgrade_ledger: env.ledger().sequence(),
            description: String::from_str(&env, "Initial deployment"),
            admin: admin.clone(),
            rollback_deadline: 0,
        };

        env.storage()
            .persistent()
            .set(&DataKey::CurrentVersion, &initial);
        env.storage()
            .persistent()
            .set(&DataKey::Version(initial_version.clone()), &initial);

        extend_ttl_for_key(&env, &DataKey::CurrentVersion);
        extend_ttl_for_key(&env, &DataKey::Version(initial_version.clone()));

        env.events().publish(
            (symbol_short!("upgrade"), symbol_short!("init")),
            UpgradeInitializedEvent {
                admin,
                version: initial_version,
                wasm_hash: initial.wasm_hash,
            },
        );

        Ok(())
    }

    /// Set a new admin for the upgrade manager.
    ///
    /// Intentionally single-signature even when a multisig is configured, so a
    /// misconfigured signer set can always be repaired.
    pub fn set_admin(env: Env, new_admin: Address) -> Result<(), UpgradeError> {
        require_not_paused(&env)?;
        let old_admin = require_admin(&env)?;
        env.storage().instance().set(&DataKey::Admin, &new_admin);

        env.events().publish(
            (symbol_short!("upgrade"), symbol_short!("adm_chng")),
            AdminChangedEvent {
                old_admin,
                new_admin,
            },
        );

        Ok(())
    }

    /// Get the current admin.
    pub fn get_admin(env: Env) -> Option<Address> {
        env.storage().instance().get(&DataKey::Admin)
    }

    /// Configure the multisig signer set, threshold, and timelock delay.
    ///
    /// Single-signature (admin) on purpose — see [`UpgradeManager::set_admin`].
    pub fn set_multisig_config(
        env: Env,
        admins: Vec<Address>,
        threshold: u32,
        timelock_delay: u64,
    ) -> Result<(), UpgradeError> {
        require_not_paused(&env)?;
        require_admin(&env)?;

        if admins.is_empty() || threshold == 0 || threshold > admins.len() {
            return Err(UpgradeError::InvalidMultisigConfig);
        }
        if timelock_delay == 0 {
            return Err(UpgradeError::InvalidMultisigConfig);
        }

        env.storage().instance().set(
            &DataKey::MultisigConfig,
            &MultisigConfig {
                admins: admins.clone(),
                threshold,
                timelock_delay,
            },
        );

        env.events().publish(
            (symbol_short!("upgrade"), symbol_short!("msig_set")),
            MultisigConfigChangedEvent {
                admins,
                threshold,
                timelock_delay,
            },
        );

        Ok(())
    }

    /// Read the multisig configuration, if one is configured.
    pub fn get_multisig_config(env: Env) -> Option<MultisigConfig> {
        get_multisig(&env)
    }

    /// Pause the contract. Only admin can call this.
    pub fn pause(env: Env) -> Result<(), UpgradeError> {
        require_admin(&env)?;
        env.storage().instance().set(&DataKey::Paused, &true);
        env.events()
            .publish((symbol_short!("upgrade"), symbol_short!("paused")), ());
        Ok(())
    }

    /// Unpause the contract. Only admin can call this.
    pub fn unpause(env: Env) -> Result<(), UpgradeError> {
        require_admin(&env)?;
        env.storage().instance().set(&DataKey::Paused, &false);
        env.events()
            .publish((symbol_short!("upgrade"), symbol_short!("unpaused")), ());
        Ok(())
    }

    /// Returns whether the contract is currently paused.
    pub fn is_paused(env: Env) -> bool {
        env.storage()
            .instance()
            .get(&DataKey::Paused)
            .unwrap_or(false)
    }

    /// Get the current contract version.
    pub fn get_current_version(env: Env) -> Option<ContractVersion> {
        get_current_version(&env)
    }

    /// Get version history for a specific version.
    pub fn get_version(env: Env, version: String) -> Option<ContractVersion> {
        env.storage().persistent().get(&DataKey::Version(version))
    }

    /// Create an upgrade proposal and start governance.
    ///
    /// Returns the new proposal id. The proposer is recorded as the first
    /// approver, so a 1-of-N configuration becomes immediately timelocked while
    /// a higher threshold stays in [`ProposalStatus::Proposed`].
    pub fn propose_upgrade(
        env: Env,
        proposer: Address,
        new_version: String,
        new_wasm_hash: BytesN<32>,
        description: String,
        migration_plan: MigrationPlan,
    ) -> Result<u64, UpgradeError> {
        require_not_paused(&env)?;
        require_governance_member(&env, &proposer)?;

        if let Some(current) = get_current_version(&env) {
            if !is_version_newer(&current.version, &new_version) {
                return Err(UpgradeError::DowngradeNotAllowed);
            }
        }

        let (threshold, timelock_delay) = effective_config(&env);
        let id = next_proposal_id(&env);
        let now = env.ledger().timestamp();

        let mut approvals = Vec::new(&env);
        approvals.push_back(proposer.clone());

        let mut proposal = UpgradeProposal {
            id,
            proposer: proposer.clone(),
            new_version: new_version.clone(),
            new_wasm_hash: new_wasm_hash.clone(),
            description: description.clone(),
            migration_plan,
            created_at: now,
            eta: 0,
            expires_at: now + DEFAULT_PROPOSAL_EXPIRY,
            approvals,
            status: ProposalStatus::Proposed,
            validated: false,
            estimated_gas: 0,
            previous_version: String::from_str(&env, ""),
            previous_wasm_hash: BytesN::from_array(&env, &[0u8; 32]),
            executed_ledger: 0,
            rollback_deadline: 0,
        };

        // Start the timelock now only if the proposer's own approval already
        // satisfies the threshold (the 1-of-N case).
        if threshold <= 1 {
            proposal.eta = now + timelock_delay;
            proposal.status = ProposalStatus::PendingTimelock;
        }

        save_proposal(&env, &proposal);
        env.storage()
            .instance()
            .set(&DataKey::NextProposalId, &(id + 1));

        env.events().publish(
            (symbol_short!("upgrade"), symbol_short!("proposed")),
            UpgradeProposedEvent {
                proposal_id: id,
                version: new_version,
                wasm_hash: new_wasm_hash,
                proposer,
                description,
                threshold,
                expires_at: proposal.expires_at,
            },
        );

        // The implicit proposer approval is a real approval and emits too, so
        // an indexer counting approvals never under-reports.
        if threshold <= 1 {
            env.events().publish(
                (symbol_short!("upgrade"), symbol_short!("approved")),
                UpgradeApprovedEvent {
                    proposal_id: id,
                    approver: proposal.proposer.clone(),
                    approval_count: 1,
                    threshold,
                    eta: proposal.eta,
                },
            );
        }

        Ok(id)
    }

    /// Record a governance member's approval.
    ///
    /// The timelock starts on the approval that *reaches the threshold*, not at
    /// proposal creation. This is an intentional, security-motivated deviation
    /// from `agent_registry`, which sets `eta` at propose time — under that
    /// scheme a proposal that sits below threshold past its `eta` becomes
    /// instantly executable the moment the final approval lands, so the timelock
    /// never actually delays anything.
    pub fn approve_upgrade(
        env: Env,
        approver: Address,
        proposal_id: u64,
    ) -> Result<(), UpgradeError> {
        require_not_paused(&env)?;
        require_governance_member(&env, &approver)?;

        let mut proposal = load_proposal(&env, proposal_id)?;
        let now = env.ledger().timestamp();

        if proposal.status == ProposalStatus::Executed
            || proposal.status == ProposalStatus::RolledBack
        {
            return Err(UpgradeError::ProposalAlreadyExecuted);
        }
        if now > proposal.expires_at {
            return Err(UpgradeError::ProposalExpired);
        }
        if proposal.approvals.contains(&approver) {
            return Err(UpgradeError::AlreadyApproved);
        }

        proposal.approvals.push_back(approver.clone());
        let count = proposal.approvals.len() as u32;

        let (threshold, timelock_delay) = effective_config(&env);
        if proposal.eta == 0 && count >= threshold {
            proposal.eta = now + timelock_delay;
            proposal.status = ProposalStatus::PendingTimelock;
        } else if count == 1 {
            proposal.status = ProposalStatus::Approved;
        }

        save_proposal(&env, &proposal);

        env.events().publish(
            (symbol_short!("upgrade"), symbol_short!("approved")),
            UpgradeApprovedEvent {
                proposal_id,
                approver,
                approval_count: count,
                threshold,
                eta: proposal.eta,
            },
        );

        Ok(())
    }

    /// Run the pre-upgrade validation hook and estimate migration gas.
    pub fn validate_proposal(
        env: Env,
        caller: Address,
        proposal_id: u64,
    ) -> Result<u64, UpgradeError> {
        require_not_paused(&env)?;
        require_governance_member(&env, &caller)?;

        let mut proposal = load_proposal(&env, proposal_id)?;
        if proposal.status == ProposalStatus::Executed
            || proposal.status == ProposalStatus::RolledBack
        {
            return Err(UpgradeError::ProposalAlreadyExecuted);
        }

        let validation_result = execute_pre_upgrade_validation(&env, &proposal)?;
        let estimated_gas = estimate_migration_gas(&env, &proposal.migration_plan);

        proposal.validated = true;
        proposal.estimated_gas = estimated_gas;
        save_proposal(&env, &proposal);

        env.events().publish(
            (symbol_short!("upgrade"), symbol_short!("validated")),
            UpgradeValidatedEvent {
                proposal_id,
                version: proposal.new_version,
                estimated_gas,
                validation_results: validation_result,
            },
        );

        Ok(estimated_gas)
    }

    /// Execute an approved, timelocked proposal: replace this contract's WASM.
    ///
    /// Requires BOTH the timelock to have elapsed AND the approval threshold to
    /// be met. The WASM swap itself is deferred by the host until this
    /// invocation finishes successfully, so the bookkeeping below still runs
    /// under the outgoing code.
    pub fn execute_upgrade(
        env: Env,
        executor: Address,
        proposal_id: u64,
    ) -> Result<(), UpgradeError> {
        require_not_paused(&env)?;
        require_governance_member(&env, &executor)?;

        let mut proposal = load_proposal(&env, proposal_id)?;
        let now = env.ledger().timestamp();
        let status = effective_status(&proposal, now);

        match status {
            ProposalStatus::Executed | ProposalStatus::RolledBack => {
                return Err(UpgradeError::ProposalAlreadyExecuted)
            }
            ProposalStatus::Expired => return Err(UpgradeError::ProposalExpired),
            ProposalStatus::PendingTimelock => return Err(UpgradeError::TimelockNotElapsed),
            ProposalStatus::Proposed | ProposalStatus::Approved => {
                return Err(UpgradeError::InsufficientApprovals)
            }
            ProposalStatus::Ready => {}
        }

        let (threshold, _) = effective_config(&env);
        if (proposal.approvals.len() as u32) < threshold {
            return Err(UpgradeError::InsufficientApprovals);
        }
        if !proposal.validated {
            return Err(UpgradeError::ProposalNotValidated);
        }

        let previous_version = get_current_version(&env);
        let rollback_deadline = env.ledger().sequence() + ROLLBACK_WINDOW_LEDGERS;
        let executor_address = executor.clone();

        // Replace this contract's own executable. No other contract can do this
        // for us — see the module docs.
        #[cfg(all(target_arch = "wasm32", not(any(test, feature = "testutils"))))]
        env.deployer()
            .update_current_contract_wasm(proposal.new_wasm_hash.clone());

        let new_version = ContractVersion {
            version: proposal.new_version.clone(),
            wasm_hash: proposal.new_wasm_hash.clone(),
            upgrade_ledger: env.ledger().sequence(),
            description: proposal.description.clone(),
            admin: executor_address.clone(),
            rollback_deadline,
        };

        env.storage()
            .persistent()
            .set(&DataKey::CurrentVersion, &new_version);
        env.storage().persistent().set(
            &DataKey::Version(proposal.new_version.clone()),
            &new_version,
        );

        if let Some(ref prev) = previous_version {
            env.storage().persistent().set(
                &DataKey::Rollback(proposal_id),
                &RollbackRecord {
                    previous_version: prev.clone(),
                    rollback_deadline,
                    can_rollback: true,
                },
            );
            extend_ttl_for_key(&env, &DataKey::Rollback(proposal_id));

            proposal.previous_version = prev.version.clone();
            proposal.previous_wasm_hash = prev.wasm_hash.clone();
        }

        proposal.status = ProposalStatus::Executed;
        proposal.executed_ledger = env.ledger().sequence();
        proposal.rollback_deadline = rollback_deadline;
        save_proposal(&env, &proposal);

        execute_post_upgrade_migration(&env, &proposal.migration_plan)?;

        extend_ttl_for_key(&env, &DataKey::CurrentVersion);
        extend_ttl_for_key(&env, &DataKey::Version(proposal.new_version.clone()));
        env.storage()
            .instance()
            .extend_ttl(TTL_THRESHOLD, TTL_EXTEND_TO);

        env.events().publish(
            (symbol_short!("upgrade"), symbol_short!("applied")),
            UpgradeAppliedEvent {
                proposal_id,
                old_version: previous_version
                    .map(|v| v.version)
                    .unwrap_or(String::from_str(&env, "none")),
                new_version: proposal.new_version,
                wasm_hash: proposal.new_wasm_hash,
                admin: executor_address,
                rollback_deadline,
            },
        );

        Ok(())
    }

    /// Revert this contract's executable to the hash captured at execution.
    ///
    /// CODE-ONLY REVERT. This restores the previous WASM; it does **not** undo
    /// any storage mutation the post-upgrade migration performed. It reuses
    /// `update_current_contract_wasm` because that is the only primitive the
    /// platform offers, so "rollback" and "upgrade" are the same operation
    /// pointed at a different hash.
    pub fn rollback_upgrade(
        env: Env,
        executor: Address,
        proposal_id: u64,
    ) -> Result<(), UpgradeError> {
        require_not_paused(&env)?;
        require_governance_member(&env, &executor)?;

        let mut proposal = load_proposal(&env, proposal_id)?;
        if proposal.status != ProposalStatus::Executed {
            return Err(UpgradeError::NoRollbackAvailable);
        }

        let record: RollbackRecord = env
            .storage()
            .persistent()
            .get(&DataKey::Rollback(proposal_id))
            .ok_or(UpgradeError::NoRollbackAvailable)?;

        if !record.can_rollback {
            return Err(UpgradeError::NoRollbackAvailable);
        }
        if env.ledger().sequence() > record.rollback_deadline {
            return Err(UpgradeError::RollbackDeadlineExpired);
        }

        let reverted_version = proposal.new_version.clone();
        let restored_wasm_hash = record.previous_version.wasm_hash.clone();
        let restored_version = record.previous_version.version.clone();

        #[cfg(all(target_arch = "wasm32", not(any(test, feature = "testutils"))))]
        env.deployer()
            .update_current_contract_wasm(restored_wasm_hash.clone());

        env.storage()
            .persistent()
            .set(&DataKey::CurrentVersion, &record.previous_version);

        // Single-use: a second rollback must not be possible.
        env.storage()
            .persistent()
            .remove(&DataKey::Rollback(proposal_id));

        proposal.status = ProposalStatus::RolledBack;
        save_proposal(&env, &proposal);

        extend_ttl_for_key(&env, &DataKey::CurrentVersion);
        env.storage()
            .instance()
            .extend_ttl(TTL_THRESHOLD, TTL_EXTEND_TO);

        env.events().publish(
            (symbol_short!("upgrade"), symbol_short!("rollback")),
            UpgradeRolledBackEvent {
                proposal_id,
                reverted_version,
                restored_version,
                restored_wasm_hash,
                admin: executor,
            },
        );

        Ok(())
    }

    /// Read a proposal, resolving the derived `Ready` status.
    pub fn get_proposal(env: Env, proposal_id: u64) -> Option<UpgradeProposal> {
        let mut proposal: UpgradeProposal = env
            .storage()
            .persistent()
            .get(&DataKey::Proposal(proposal_id))?;
        proposal.status = effective_status(&proposal, env.ledger().timestamp());
        Some(proposal)
    }

    /// Get rollback information for a proposal, if one is recorded.
    pub fn get_rollback_info(env: Env, proposal_id: u64) -> Option<RollbackRecord> {
        env.storage()
            .persistent()
            .get(&DataKey::Rollback(proposal_id))
    }

    /// Whether the given proposal can still be rolled back.
    pub fn can_rollback(env: Env, proposal_id: u64) -> bool {
        match env
            .storage()
            .persistent()
            .get::<DataKey, RollbackRecord>(&DataKey::Rollback(proposal_id))
        {
            Some(record) => {
                record.can_rollback && env.ledger().sequence() <= record.rollback_deadline
            }
            None => false,
        }
    }

    /// Mark expired, non-terminal proposals as `Expired`.
    ///
    /// Bounded by [`MAX_SWEEP_PROPOSALS`] to cap gas. Returns how many
    /// proposals were transitioned.
    pub fn sweep_expired_proposals(env: Env, caller: Address) -> Result<u32, UpgradeError> {
        require_governance_member(&env, &caller)?;
        let admin = caller.clone();
        let now = env.ledger().timestamp();
        let next_id = next_proposal_id(&env);
        let mut swept = 0u32;

        let mut id = 1u64;
        while id < next_id && swept < MAX_SWEEP_PROPOSALS {
            if let Some(mut proposal) = env
                .storage()
                .persistent()
                .get::<DataKey, UpgradeProposal>(&DataKey::Proposal(id))
            {
                if !proposal.status.is_terminal() && now > proposal.expires_at {
                    proposal.status = ProposalStatus::Expired;
                    save_proposal(&env, &proposal);
                    swept += 1;
                }
            }
            id += 1;
        }

        env.events().publish(
            (symbol_short!("upgrade"), symbol_short!("swept")),
            ExpiredProposalsSweptEvent { swept, admin },
        );

        Ok(swept)
    }

    /// Estimate gas costs for a migration plan.
    pub fn estimate_migration_gas(env: Env, migration_plan: MigrationPlan) -> u64 {
        estimate_migration_gas(&env, &migration_plan)
    }

    /// Get the proposal id that will be assigned to the next proposal.
    pub fn next_proposal_id(env: Env) -> u64 {
        next_proposal_id(&env)
    }

    /// Get all version history (for debugging/auditing).
    pub fn get_version_history(env: Env) -> Vec<ContractVersion> {
        let mut history = Vec::new(&env);
        if let Some(current) = get_current_version(&env) {
            history.push_back(current);
        }
        history
    }
}

#[cfg(test)]
mod tests;

fn estimate_migration_gas(_env: &Env, migration_plan: &MigrationPlan) -> u64 {
    let base_cost = GAS_UPGRADE_BASE;
    let item_cost = GAS_MIGRATION_PER_ITEM * migration_plan.estimated_items as u64;

    // Add overhead for each migration step
    let step_overhead = (migration_plan.pre_migration_checks.len()
        + migration_plan.data_transformations.len()
        + migration_plan.post_migration_validations.len()) as u64
        * 5000;

    base_cost + item_cost + step_overhead
}

#![no_std]

//! # Upgrade Manager Contract
//!
//! Provides safe contract upgrade functionality with version tracking, data migration hooks,
//! and rollback capabilities for Soroban contracts.
//!
//! ## Features
//!
//! - **Version Tracking**: Each contract version is stored with metadata
//! - **Migration Hooks**: Pre-upgrade validation and post-upgrade data migration
//! - **Rollback Support**: Admin can revert to previous version within 48h window
//! - **Gas Budget Estimation**: Calculate gas costs for migration operations
//! - **Event System**: Comprehensive upgrade tracking via events
//! - **Admin Controls**: Only authorized admins can perform upgrades
//!
//! ## Usage Flow
//!
//! 1. `propose_upgrade` - Admin proposes new WASM hash with validation
//! 2. Pre-upgrade hook validates compatibility and estimates gas
//! 3. `execute_upgrade` - Admin executes the upgrade after validation
//! 4. Post-upgrade hook migrates data to new format
//! 5. `rollback_upgrade` - Optional rollback within 48h window
//!
//! ## Security Model
//!
//! - Only contract admin can propose/execute upgrades
//! - Pre-upgrade hooks prevent incompatible upgrades
//! - Rollback window provides safety net for problematic upgrades
//! - Version tracking prevents downgrades without explicit rollback

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

/// Rollback window in ledgers (48 hours at ~5s per ledger)
pub const ROLLBACK_WINDOW_LEDGERS: u32 = 34_560;

/// Default TTL threshold for storage extension
pub const TTL_THRESHOLD: u32 = 100_000;
/// Target TTL after extension (~31 days)
pub const TTL_EXTEND_TO: u32 = 535_680;

/// Gas budget constants for upgrade operations
pub const GAS_UPGRADE_BASE: u64 = 500_000;
pub const GAS_MIGRATION_PER_ITEM: u64 = 10_000;
pub const GAS_ROLLBACK_BASE: u64 = 200_000;
/// Fixed overhead charged per migration step (check, transformation or validation)
pub const GAS_MIGRATION_STEP_OVERHEAD: u64 = 5_000;

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

/// Upgrade proposal before execution
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UpgradeProposal {
    pub new_version: String,
    pub new_wasm_hash: BytesN<32>,
    pub description: String,
    pub proposed_ledger: u32,
    pub proposer: Address,
    pub validated: bool,
    pub estimated_gas: u64,
    pub migration_plan: MigrationPlan,
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

/// Storage keys for upgrade manager data
#[contracttype]
#[derive(Clone)]
pub enum DataKey {
    /// Current admin address
    Admin,
    /// Whether the contract is paused
    Paused,
    /// Current contract version
    CurrentVersion,
    /// Version history (version_string -> ContractVersion)
    Version(String),
    /// Active upgrade proposal
    Proposal,
    /// Rollback information
    Rollback,
    /// Migration state during upgrade
    MigrationState,
    /// Contract-specific upgrade hooks
    UpgradeHooks,
}

/// Upgrade operation errors
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
    /// A version tag is not a well-formed `MAJOR.MINOR.PATCH[-pre][+build]`
    MalformedVersion = 14,
    /// The Wasm swap is not available in this build, so the upgrade could
    /// not actually be applied
    SwapUnavailable = 15,
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

/// Returns `Ok(true)` when `proposed` has strictly higher semver precedence
/// than `current`, using [`strutil::compare_versions`] (numeric component-wise
/// comparison; pre-releases sort below their release). Malformed tags yield
/// [`UpgradeError::MalformedVersion`] instead of falling back to byte order.
fn is_version_newer(current: &String, proposed: &String) -> Result<bool, UpgradeError> {
    strutil::compare_versions(proposed, current)
        .map(|ord| ord == core::cmp::Ordering::Greater)
        .ok_or(UpgradeError::MalformedVersion)
}

// ─── Contract Implementation ─────────────────────────────────────────────────

#[cfg(feature = "contract")]
#[contractimpl]
impl UpgradeManager {
    /// Initialize the upgrade manager with an admin and initial version
    pub fn initialize(
        env: Env,
        admin: Address,
        initial_version: String,
        initial_wasm_hash: BytesN<32>,
    ) -> Result<(), UpgradeError> {
        admin.require_auth();
        if env.storage().instance().has(&DataKey::Admin) {
            return Err(UpgradeError::InvalidVersion);
        }

        env.storage().instance().set(&DataKey::Admin, &admin);
        env.storage().instance().set(&DataKey::Paused, &false);
        record_wasm_hash(&env, &initial_wasm_hash);

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

    /// Set a new admin for the upgrade manager
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

    /// Get the current admin
    pub fn get_admin(env: Env) -> Option<Address> {
        env.storage().instance().get(&DataKey::Admin)
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

    /// Get the current contract version
    pub fn get_current_version(env: Env) -> Option<ContractVersion> {
        get_current_version(&env)
    }

    /// Get version history for a specific version
    pub fn get_version(env: Env, version: String) -> Option<ContractVersion> {
        env.storage().persistent().get(&DataKey::Version(version))
    }

    /// Propose a new upgrade with validation
    pub fn propose_upgrade(
        env: Env,
        new_version: String,
        new_wasm_hash: BytesN<32>,
        description: String,
        migration_plan: MigrationPlan,
    ) -> Result<(), UpgradeError> {
        require_not_paused(&env)?;
        let admin = require_admin(&env)?;

        // Check if version is valid and newer
        if let Some(current) = get_current_version(&env) {
            if !is_version_newer(&current.version, &new_version)? {
                return Err(UpgradeError::DowngradeNotAllowed);
            }
        } else if strutil::compare_versions(&new_version, &new_version).is_none() {
            return Err(UpgradeError::MalformedVersion);
        }

        // Create proposal
        let proposal = UpgradeProposal {
            new_version: new_version.clone(),
            new_wasm_hash: new_wasm_hash.clone(),
            description: description.clone(),
            proposed_ledger: env.ledger().sequence(),
            proposer: admin,
            validated: false,
            estimated_gas: 0,
            migration_plan,
        };

        env.storage()
            .persistent()
            .set(&DataKey::Proposal, &proposal);
        extend_ttl_for_key(&env, &DataKey::Proposal);

        env.events().publish(
            (symbol_short!("upgrade"), symbol_short!("proposed")),
            UpgradeProposedEvent {
                version: new_version,
                wasm_hash: new_wasm_hash,
                proposer: proposal.proposer,
                description,
            },
        );

        Ok(())
    }

    /// Validate the current upgrade proposal (pre-upgrade hook)
    pub fn validate_proposal(env: Env) -> Result<u64, UpgradeError> {
        require_not_paused(&env)?;
        require_admin(&env)?;

        let mut proposal: UpgradeProposal = env
            .storage()
            .persistent()
            .get(&DataKey::Proposal)
            .ok_or(UpgradeError::NoProposal)?;

        // Execute pre-upgrade validation
        let validation_result = execute_pre_upgrade_validation(&env, &proposal)?;

        // Estimate gas costs
        let estimated_gas = estimate_migration_gas(&env, &proposal.migration_plan);

        proposal.validated = true;
        proposal.estimated_gas = estimated_gas;

        env.storage()
            .persistent()
            .set(&DataKey::Proposal, &proposal);
        extend_ttl_for_key(&env, &DataKey::Proposal);

        env.events().publish(
            (symbol_short!("upgrade"), symbol_short!("validated")),
            UpgradeValidatedEvent {
                version: proposal.new_version,
                estimated_gas,
                validation_results: validation_result,
            },
        );

        Ok(estimated_gas)
    }

    /// Execute the validated upgrade proposal
    pub fn execute_upgrade(env: Env) -> Result<(), UpgradeError> {
        require_not_paused(&env)?;
        let admin = require_admin(&env)?;

        let proposal: UpgradeProposal = env
            .storage()
            .persistent()
            .get(&DataKey::Proposal)
            .ok_or(UpgradeError::NoProposal)?;

        if !proposal.validated {
            return Err(UpgradeError::ProposalNotValidated);
        }

        // Store current version for potential rollback
        let current_version = get_current_version(&env);
        let rollback_deadline = env.ledger().sequence() + ROLLBACK_WINDOW_LEDGERS;

        // Execute the upgrade. Fails with `SwapUnavailable` rather than
        // reporting success when the swap cannot be performed.
        swap_wasm(&env, &proposal.new_wasm_hash).map_err(|_| UpgradeError::SwapUnavailable)?;

        // Create new version record
        let new_version = ContractVersion {
            version: proposal.new_version.clone(),
            wasm_hash: proposal.new_wasm_hash.clone(),
            upgrade_ledger: env.ledger().sequence(),
            description: proposal.description.clone(),
            admin,
            rollback_deadline,
        };

        // Update storage
        env.storage()
            .persistent()
            .set(&DataKey::CurrentVersion, &new_version);
        env.storage().persistent().set(
            &DataKey::Version(proposal.new_version.clone()),
            &new_version,
        );

        // Store rollback info if we had a previous version
        if let Some(ref prev_version) = current_version {
            let rollback_record = RollbackRecord {
                previous_version: prev_version.clone(),
                rollback_deadline,
                can_rollback: true,
            };
            env.storage()
                .persistent()
                .set(&DataKey::Rollback, &rollback_record);
            extend_ttl_for_key(&env, &DataKey::Rollback);
        }

        // Clean up proposal
        env.storage().persistent().remove(&DataKey::Proposal);

        // Execute post-upgrade migration
        execute_post_upgrade_migration(&env, &proposal.migration_plan)?;

        extend_ttl_for_key(&env, &DataKey::CurrentVersion);
        extend_ttl_for_key(&env, &DataKey::Version(proposal.new_version.clone()));
        env.storage()
            .instance()
            .extend_ttl(TTL_THRESHOLD, TTL_EXTEND_TO);

        env.events().publish(
            (symbol_short!("upgrade"), symbol_short!("applied")),
            UpgradeAppliedEvent {
                old_version: current_version
                    .map(|v| v.version)
                    .unwrap_or(String::from_str(&env, "none")),
                new_version: proposal.new_version,
                wasm_hash: proposal.new_wasm_hash.clone(),
                admin: new_version.admin,
            },
        );

        Ok(())
    }

    /// Rollback to the previous version (within 48h window)
    pub fn rollback_upgrade(env: Env) -> Result<(), UpgradeError> {
        require_not_paused(&env)?;
        let admin = require_admin(&env)?;

        let rollback_record: RollbackRecord = env
            .storage()
            .persistent()
            .get(&DataKey::Rollback)
            .ok_or(UpgradeError::NoRollbackAvailable)?;

        if !rollback_record.can_rollback {
            return Err(UpgradeError::NoRollbackAvailable);
        }

        if env.ledger().sequence() > rollback_record.rollback_deadline {
            return Err(UpgradeError::RollbackDeadlineExpired);
        }

        let current_version = get_current_version(&env).unwrap();

        // Perform the rollback
        swap_wasm(&env, &rollback_record.previous_version.wasm_hash)
            .map_err(|_| UpgradeError::SwapUnavailable)?;

        // Restore previous version as current
        env.storage()
            .persistent()
            .set(&DataKey::CurrentVersion, &rollback_record.previous_version);

        // Disable further rollbacks
        env.storage().persistent().remove(&DataKey::Rollback);

        extend_ttl_for_key(&env, &DataKey::CurrentVersion);
        env.storage()
            .instance()
            .extend_ttl(TTL_THRESHOLD, TTL_EXTEND_TO);

        env.events().publish(
            (symbol_short!("upgrade"), symbol_short!("rollback")),
            UpgradeRolledBackEvent {
                reverted_version: current_version.version,
                restored_version: rollback_record.previous_version.version,
                admin,
            },
        );

        Ok(())
    }

    /// Get rollback information (if available)
    pub fn get_rollback_info(env: Env) -> Option<RollbackRecord> {
        env.storage().persistent().get(&DataKey::Rollback)
    }

    /// Check if rollback is still available
    pub fn can_rollback(env: Env) -> bool {
        if let Some(rollback_record) = env
            .storage()
            .persistent()
            .get::<DataKey, RollbackRecord>(&DataKey::Rollback)
        {
            rollback_record.can_rollback
                && env.ledger().sequence() <= rollback_record.rollback_deadline
        } else {
            false
        }
    }

    /// Estimate gas costs for a migration plan
    pub fn estimate_migration_gas(env: Env, migration_plan: MigrationPlan) -> u64 {
        estimate_migration_gas(&env, &migration_plan)
    }

    /// Get all version history (for debugging/auditing)
    pub fn get_version_history(env: Env) -> Vec<ContractVersion> {
        // In a real implementation, you'd maintain a separate index
        // For now, this is a placeholder that returns current version only
        let mut history = Vec::new(&env);
        if let Some(current) = get_current_version(&env) {
            history.push_back(current);
        }
        history
    }
}

// ─── Gas Estimation Functions ───────────────────────────────────────────────

fn estimate_migration_gas(_env: &Env, migration_plan: &MigrationPlan) -> u64 {
    let base_cost = GAS_UPGRADE_BASE;
    let item_cost = GAS_MIGRATION_PER_ITEM * migration_plan.estimated_items as u64;

    // Add overhead for each migration step
    let step_overhead = (migration_plan.pre_migration_checks.len()
        + migration_plan.data_transformations.len()
        + migration_plan.post_migration_validations.len()) as u64
        * GAS_MIGRATION_STEP_OVERHEAD;

    base_cost + item_cost + step_overhead
}

#[cfg(test)]
mod tests {
    use super::*;
    use soroban_sdk::{
        testutils::{Address as _, Ledger as _},
        BytesN, Env,
    };

    fn create_test_env() -> (Env, UpgradeManagerClient<'static>, Address) {
        let env = Env::default();
        env.ledger().set_sequence_number(1);
        env.mock_all_auths();
        let contract_id = env.register(UpgradeManager, ());
        let client = UpgradeManagerClient::new(&env, &contract_id);
        let admin = Address::generate(&env);
        (env, client, admin)
    }

    fn test_wasm_hash(env: &Env, seed: u8) -> BytesN<32> {
        let mut bytes = [0u8; 32];
        bytes[0] = seed;
        BytesN::from_array(env, &bytes)
    }

    #[test]
    fn test_initialize_upgrade_manager() {
        let (env, client, admin) = create_test_env();
        let initial_hash = test_wasm_hash(&env, 1);

        client.initialize(&admin, &String::from_str(&env, "1.0.0"), &initial_hash);

        assert_eq!(client.get_admin(), Some(admin));

        let version = client.get_current_version().unwrap();
        assert_eq!(version.version, String::from_str(&env, "1.0.0"));
        assert_eq!(version.wasm_hash, initial_hash);
    }

    #[test]
    fn test_propose_and_execute_upgrade() {
        let (env, client, admin) = create_test_env();
        let initial_hash = test_wasm_hash(&env, 1);
        let new_hash = test_wasm_hash(&env, 2);

        client.initialize(&admin, &String::from_str(&env, "1.0.0"), &initial_hash);

        let migration_plan = MigrationPlan {
            pre_migration_checks: Vec::new(&env),
            data_transformations: Vec::new(&env),
            post_migration_validations: Vec::new(&env),
            estimated_items: 10,
        };

        // Propose upgrade
        let result = client.try_propose_upgrade(
            &String::from_str(&env, "2.0.0"),
            &new_hash,
            &String::from_str(&env, "Major upgrade"),
            &migration_plan,
        );
        assert!(result.is_ok());

        // Validate proposal
        let gas_estimate = client.validate_proposal();
        assert!(gas_estimate > 0);

        // Execute upgrade
        let result = client.try_execute_upgrade();
        assert!(result.is_ok());

        let new_version = client.get_current_version().unwrap();
        assert_eq!(new_version.version, String::from_str(&env, "2.0.0"));
        assert_eq!(new_version.wasm_hash, new_hash);
    }

    #[test]
    fn test_rollback_within_window() {
        let (env, client, admin) = create_test_env();
        let initial_hash = test_wasm_hash(&env, 1);
        let new_hash = test_wasm_hash(&env, 2);

        client.initialize(&admin, &String::from_str(&env, "1.0.0"), &initial_hash);

        let migration_plan = MigrationPlan {
            pre_migration_checks: Vec::new(&env),
            data_transformations: Vec::new(&env),
            post_migration_validations: Vec::new(&env),
            estimated_items: 5,
        };

        // Perform upgrade
        client.propose_upgrade(
            &String::from_str(&env, "2.0.0"),
            &new_hash,
            &String::from_str(&env, "Test upgrade"),
            &migration_plan,
        );
        client.validate_proposal();
        client.execute_upgrade();

        // Check rollback is available
        assert!(client.can_rollback());

        // Perform rollback
        let result = client.try_rollback_upgrade();
        assert!(result.is_ok());

        // Verify we're back to original version
        let current = client.get_current_version().unwrap();
        assert_eq!(current.version, String::from_str(&env, "1.0.0"));
        assert_eq!(current.wasm_hash, initial_hash);

        // Verify rollback is no longer available
        assert!(!client.can_rollback());
    }

    #[test]
    fn test_rollback_after_deadline() {
        let (env, client, admin) = create_test_env();
        let initial_hash = test_wasm_hash(&env, 1);
        let new_hash = test_wasm_hash(&env, 2);

        client.initialize(&admin, &String::from_str(&env, "1.0.0"), &initial_hash);

        let migration_plan = MigrationPlan {
            pre_migration_checks: Vec::new(&env),
            data_transformations: Vec::new(&env),
            post_migration_validations: Vec::new(&env),
            estimated_items: 5,
        };

        // Perform upgrade
        client.propose_upgrade(
            &String::from_str(&env, "2.0.0"),
            &new_hash,
            &String::from_str(&env, "Test upgrade"),
            &migration_plan,
        );
        client.validate_proposal();
        client.execute_upgrade();

        // Advance ledger past rollback deadline
        let current_seq = env.ledger().sequence();
        env.ledger()
            .set_sequence_number(current_seq + ROLLBACK_WINDOW_LEDGERS + 1);

        // Rollback should fail
        let result = client.try_rollback_upgrade();
        assert_eq!(result, Err(Ok(UpgradeError::RollbackDeadlineExpired)));
    }

    fn empty_plan(env: &Env) -> MigrationPlan {
        MigrationPlan {
            pre_migration_checks: Vec::new(env),
            data_transformations: Vec::new(env),
            post_migration_validations: Vec::new(env),
            estimated_items: 0,
        }
    }

    #[test]
    fn propose_upgrade_uses_semver_precedence() {
        // (current, proposed, expected result of propose_upgrade)
        let cases: &[(&str, &str, Result<(), UpgradeError>)] = &[
            ("1.9.0", "1.10.0", Ok(())),
            ("1.99.0", "1.100.0", Ok(())),
            ("2.0.0", "10.0.0", Ok(())),
            ("1.0.0-rc.1", "1.0.0", Ok(())),
            ("1.0.0", "1.0.0-rc.1", Err(UpgradeError::DowngradeNotAllowed)),
            ("1.0.0", "1.0.0", Err(UpgradeError::DowngradeNotAllowed)),
            ("1.0.1", "1.0.0", Err(UpgradeError::DowngradeNotAllowed)),
            ("1.0", "1.0.0.1", Ok(())),
            ("1.0.0", "1.x.0", Err(UpgradeError::MalformedVersion)),
            ("1.0.0", "", Err(UpgradeError::MalformedVersion)),
        ];
        for (current, proposed, expected) in cases {
            let (env, client, admin) = create_test_env();
            client.initialize(&admin, &String::from_str(&env, current), &test_wasm_hash(&env, 1));
            let result = client.try_propose_upgrade(
                &String::from_str(&env, proposed),
                &test_wasm_hash(&env, 2),
                &String::from_str(&env, "upgrade"),
                &empty_plan(&env),
            );
            let actual = match result {
                Ok(_) => Ok(()),
                Err(Ok(e)) => Err(e),
                Err(Err(_)) => panic!("host error"),
            };
            assert_eq!(actual, *expected, "{} -> {}", current, proposed);
        }
    }

    #[test]
    fn execute_upgrade_invokes_swap_with_proposed_hash() {
        let (env, client, admin) = create_test_env();
        let initial_hash = test_wasm_hash(&env, 1);
        let new_hash = test_wasm_hash(&env, 2);
        client.initialize(&admin, &String::from_str(&env, "1.9.0"), &initial_hash);

        let calls = env.as_contract(&client.address, || mock_swap_calls(&env));
        assert_eq!(calls.len(), 0);
        assert_eq!(
            env.as_contract(&client.address, || stored_wasm_hash(&env)),
            Some(initial_hash.clone())
        );

        client.propose_upgrade(
            &String::from_str(&env, "1.10.0"),
            &new_hash,
            &String::from_str(&env, "upgrade"),
            &empty_plan(&env),
        );
        client.validate_proposal();
        client.execute_upgrade();

        let calls = env.as_contract(&client.address, || mock_swap_calls(&env));
        assert_eq!(calls.len(), 1);
        assert_eq!(calls.get(0).unwrap(), new_hash);
        assert_eq!(
            env.as_contract(&client.address, || stored_wasm_hash(&env)),
            Some(new_hash)
        );

        client.rollback_upgrade();
        let calls = env.as_contract(&client.address, || mock_swap_calls(&env));
        assert_eq!(calls.len(), 2);
        assert_eq!(calls.get(1).unwrap(), initial_hash);
    }

    #[test]
    fn test_negative_auth_initialize() {
        let env = Env::default();
        env.ledger().set_sequence_number(1);
        env.mock_auths(&[]);
        let contract_id = env.register(UpgradeManager, ());
        let client = UpgradeManagerClient::new(&env, &contract_id);
        let admin = Address::generate(&env);
        let initial_hash = test_wasm_hash(&env, 1);
        assert!(client
            .try_initialize(&admin, &String::from_str(&env, "1.0.0"), &initial_hash)
            .is_err());
    }

    #[test]
    fn test_gas_estimation() {
        let (env, client, _admin) = create_test_env();

        let mut migration_plan = MigrationPlan {
            pre_migration_checks: Vec::new(&env),
            data_transformations: Vec::new(&env),
            post_migration_validations: Vec::new(&env),
            estimated_items: 100,
        };

        migration_plan
            .pre_migration_checks
            .push_back(String::from_str(&env, "check1"));
        migration_plan
            .data_transformations
            .push_back(String::from_str(&env, "transform1"));
        migration_plan
            .post_migration_validations
            .push_back(String::from_str(&env, "validate1"));

        let gas_estimate = client.estimate_migration_gas(&migration_plan);

        let expected = GAS_UPGRADE_BASE + (GAS_MIGRATION_PER_ITEM * 100) + (3 * 5000);
        assert_eq!(gas_estimate, expected);
    }

    // ========================================================================
    // Negative Authorization Tests (Issue #549)
    // ========================================================================

    #[test]
    fn negative_auth_set_admin() {
        let (env, client, admin) = create_test_env();
        let initial_hash = test_wasm_hash(&env, 1);
        client.initialize(&admin, &String::from_str(&env, "1.0.0"), &initial_hash);

        let intruder = Address::generate(&env);
        env.mock_auths(&[]);

        let result = client.try_set_admin(&intruder);
        assert_eq!(result, Err(Ok(UpgradeError::Unauthorized)));
    }

    #[test]
    fn negative_auth_pause() {
        let (env, client, admin) = create_test_env();
        let initial_hash = test_wasm_hash(&env, 1);
        client.initialize(&admin, &String::from_str(&env, "1.0.0"), &initial_hash);

        env.mock_auths(&[]);

        let result = client.try_pause();
        assert_eq!(result, Err(Ok(UpgradeError::Unauthorized)));
    }

    #[test]
    fn negative_auth_unpause() {
        let (env, client, admin) = create_test_env();
        let initial_hash = test_wasm_hash(&env, 1);
        client.initialize(&admin, &String::from_str(&env, "1.0.0"), &initial_hash);

        env.mock_all_auths();
        client.pause();

        env.mock_auths(&[]);

        let result = client.try_unpause();
        assert_eq!(result, Err(Ok(UpgradeError::Unauthorized)));
    }

    #[test]
    fn negative_auth_propose_upgrade() {
        let (env, client, admin) = create_test_env();
        let initial_hash = test_wasm_hash(&env, 1);
        let new_hash = test_wasm_hash(&env, 2);
        client.initialize(&admin, &String::from_str(&env, "1.0.0"), &initial_hash);

        let migration_plan = MigrationPlan {
            pre_migration_checks: Vec::new(&env),
            data_transformations: Vec::new(&env),
            post_migration_validations: Vec::new(&env),
            estimated_items: 10,
        };

        env.mock_auths(&[]);

        let result = client.try_propose_upgrade(
            &String::from_str(&env, "2.0.0"),
            &new_hash,
            &String::from_str(&env, "Unauthorized upgrade"),
            &migration_plan,
        );
        assert_eq!(result, Err(Ok(UpgradeError::Unauthorized)));
    }

    #[test]
    fn negative_auth_validate_proposal() {
        let (env, client, admin) = create_test_env();
        let initial_hash = test_wasm_hash(&env, 1);
        let new_hash = test_wasm_hash(&env, 2);
        client.initialize(&admin, &String::from_str(&env, "1.0.0"), &initial_hash);

        let migration_plan = MigrationPlan {
            pre_migration_checks: Vec::new(&env),
            data_transformations: Vec::new(&env),
            post_migration_validations: Vec::new(&env),
            estimated_items: 10,
        };

        env.mock_all_auths();
        client.propose_upgrade(
            &String::from_str(&env, "2.0.0"),
            &new_hash,
            &String::from_str(&env, "Test upgrade"),
            &migration_plan,
        );

        env.mock_auths(&[]);

        let result = client.try_validate_proposal();
        assert_eq!(result, Err(Ok(UpgradeError::Unauthorized)));
    }

    #[test]
    fn negative_auth_execute_upgrade() {
        let (env, client, admin) = create_test_env();
        let initial_hash = test_wasm_hash(&env, 1);
        let new_hash = test_wasm_hash(&env, 2);
        client.initialize(&admin, &String::from_str(&env, "1.0.0"), &initial_hash);

        let migration_plan = MigrationPlan {
            pre_migration_checks: Vec::new(&env),
            data_transformations: Vec::new(&env),
            post_migration_validations: Vec::new(&env),
            estimated_items: 10,
        };

        env.mock_all_auths();
        client.propose_upgrade(
            &String::from_str(&env, "2.0.0"),
            &new_hash,
            &String::from_str(&env, "Test upgrade"),
            &migration_plan,
        );
        client.validate_proposal();

        env.mock_auths(&[]);

        let result = client.try_execute_upgrade();
        assert_eq!(result, Err(Ok(UpgradeError::Unauthorized)));
    }

    #[test]
    fn negative_auth_rollback_upgrade() {
        let (env, client, admin) = create_test_env();
        let initial_hash = test_wasm_hash(&env, 1);
        let new_hash = test_wasm_hash(&env, 2);
        client.initialize(&admin, &String::from_str(&env, "1.0.0"), &initial_hash);

        let migration_plan = MigrationPlan {
            pre_migration_checks: Vec::new(&env),
            data_transformations: Vec::new(&env),
            post_migration_validations: Vec::new(&env),
            estimated_items: 5,
        };

        env.mock_all_auths();
        client.propose_upgrade(
            &String::from_str(&env, "2.0.0"),
            &new_hash,
            &String::from_str(&env, "Test upgrade"),
            &migration_plan,
        );
        client.validate_proposal();
        client.execute_upgrade();

        env.mock_auths(&[]);

        let result = client.try_rollback_upgrade();
        assert_eq!(result, Err(Ok(UpgradeError::Unauthorized)));
    }
}

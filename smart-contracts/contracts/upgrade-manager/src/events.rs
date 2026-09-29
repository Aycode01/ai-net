use soroban_sdk::{contracttype, Address, BytesN, String, Vec};

/// Event emitted when upgrade manager is initialized
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UpgradeInitializedEvent {
    pub admin: Address,
    pub version: String,
    pub wasm_hash: BytesN<32>,
}

/// Event emitted when admin is changed
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AdminChangedEvent {
    pub old_admin: Address,
    pub new_admin: Address,
}

/// Event emitted when upgrade is proposed
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UpgradeProposedEvent {
    pub version: String,
    pub wasm_hash: BytesN<32>,
    pub proposer: Address,
    pub description: String,
}

/// Event emitted when upgrade proposal is validated
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UpgradeValidatedEvent {
    pub version: String,
    pub estimated_gas: u64,
    pub validation_results: Vec<String>,
}

/// Event emitted when upgrade is applied
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UpgradeAppliedEvent {
    pub old_version: String,
    pub new_version: String,
    pub wasm_hash: BytesN<32>,
    pub admin: Address,
}

/// Event emitted when upgrade is rolled back
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UpgradeRolledBackEvent {
    pub reverted_version: String,
    pub restored_version: String,
    pub admin: Address,
}

/// Event emitted when the upgrade-manager side of a migration completes.
///
/// Data transformations and post-migration validations are executed by the
/// upgraded contract's `post_upgrade_hook`; they are listed here as
/// delegated, not as performed.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MigrationCompleteEvent {
    /// The version that was applied.
    pub version: String,
    /// Transformation steps delegated to the upgraded contract.
    pub delegated_transformations: Vec<String>,
    /// Post-migration validations delegated to the upgraded contract.
    pub delegated_validations: Vec<String>,
    /// Item count declared in the migration plan (an estimate, not a count).
    pub estimated_items: u32,
}

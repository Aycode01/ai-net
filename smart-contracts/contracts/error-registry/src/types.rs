//! # Data Types for Error Registry

use soroban_sdk::{contracttype, Address, BytesN, String, Symbol};

/// A single error report stored on-chain.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ErrorRecord {
    pub error_code: u32,
    pub message: Symbol,
    pub agent_id: Symbol,
    pub created_at: u64,
    pub expires_at: u64,
}

/// Statistics returned by cleanup_expired_errors.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CleanupStats {
    pub scanned: u32,
    pub removed: u32,
    pub remaining: u32,
}

/// Detailed record of a contract upgrade or rollback event stored on-chain.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UpgradeRecord {
    pub old_version: String,
    pub new_version: String,
    pub wasm_hash: BytesN<32>,
    pub admin: Address,
    pub upgrade_ledger: u32,
    pub timestamp: u64,
}

/// Version tracking metadata for contract upgrades.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct VersionInfo {
    pub version: String,
    pub build_hash: BytesN<32>,
    pub updated_at: u64,
}

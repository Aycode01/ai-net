use soroban_sdk::{contracttype, Address, BytesN, String};

/// Upgrade tracking record
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UpgradeRecord {
    pub version: String,
    pub wasm_hash: BytesN<32>,
    pub upgraded_at: u64,
    pub admin: Address,
}

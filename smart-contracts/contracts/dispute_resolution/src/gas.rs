//! CPU instruction estimates for dispute resolution operations.
use soroban_sdk::{symbol_short, Symbol};

pub const GAS_FILE_DISPUTE_BASE: u64 = 45_000;
pub const GAS_FILE_DISPUTE_PER_JUROR: u64 = 8_000;

pub fn estimate(operation: Symbol, juror_count: u32) -> u64 {
    if operation != symbol_short!("dispute") && operation != symbol_short!("file_disp") {
        return 0;
    }

    GAS_FILE_DISPUTE_BASE
        .saturating_add(GAS_FILE_DISPUTE_PER_JUROR.saturating_mul(juror_count.min(5) as u64))
}

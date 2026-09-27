#![no_std]

//! # Dispute Resolution Contract
//!
//! On-chain dispute resolution with evidence submission, juror voting,
//! and resolution enforcement.

mod errors;
mod types;

pub use errors::Error;
pub use types::*;

use soroban_sdk::xdr::ToXdr;
use soroban_sdk::{
    contract, contractimpl, contracttype, symbol_short, Address, Bytes, BytesN, Env, Symbol, Vec,
};

/// Evidence submission phase: 3 days (259,200 seconds).
pub const EVIDENCE_PHASE: u64 = 259_200;
/// Voting phase: 2 days (172,800 seconds).
pub const VOTING_PHASE: u64 = 172_800;
/// Appeal window: 2 days (172,800 seconds).
pub const APPEAL_WINDOW: u64 = 172_800;
/// Total dispute lifecycle window.
pub const DISPUTE_WINDOW: u64 = EVIDENCE_PHASE + VOTING_PHASE + APPEAL_WINDOW;
/// Number of jurors randomly selected.
pub const JUROR_COUNT: u32 = 5;

#[contracttype]
#[derive(Clone)]
pub enum DataKey {
    Admin,
    Paused,
    Dispute(Symbol),
    Evidence(Symbol, u32),
    JurorVote(Symbol, Address),
    ActiveJurors,
    JurorPool,
}

#[contract]
pub struct DisputeResolutionContract;

fn require_admin(env: &Env) -> Result<Address, Error> {
    let admin: Address = env
        .storage()
        .instance()
        .get(&DataKey::Admin)
        .ok_or(Error::Unauthorized)?;
    admin.require_auth();
    Ok(admin)
}

fn require_not_paused(env: &Env) -> Result<(), Error> {
    let paused: bool = env
        .storage()
        .instance()
        .get(&DataKey::Paused)
        .unwrap_or(false);
    if paused {
        return Err(Error::ContractPaused);
    }
    Ok(())
}

#[contractimpl]
impl DisputeResolutionContract {
    /// Initialize the dispute resolution contract.
    pub fn initialize(env: Env, admin: Address) -> Result<(), Error> {
        if env.storage().instance().has(&DataKey::Admin) {
            return Err(Error::AlreadyExists);
        }
        env.storage().instance().set(&DataKey::Admin, &admin);
        env.storage().instance().set(&DataKey::Paused, &false);
        Ok(())
    }

    /// Admin: pause.
    pub fn pause(env: Env) -> Result<(), Error> {
        require_admin(&env)?;
        env.storage().instance().set(&DataKey::Paused, &true);
        env.events()
            .publish((symbol_short!("dispute"), symbol_short!("paused")), ());
        Ok(())
    }

    /// Admin: unpause.
    pub fn unpause(env: Env) -> Result<(), Error> {
        require_admin(&env)?;
        env.storage().instance().set(&DataKey::Paused, &false);
        env.events()
            .publish((symbol_short!("dispute"), symbol_short!("unpaused")), ());
        Ok(())
    }

    /// Returns whether the contract is currently paused.
    pub fn is_paused(env: Env) -> bool {
        env.storage()
            .instance()
            .get(&DataKey::Paused)
            .unwrap_or(false)
    }

    /// Admin: set the available juror pool.
    pub fn set_jurors(env: Env, jurors: Vec<Address>) -> Result<(), Error> {
        require_not_paused(&env)?;
        require_admin(&env)?;
        env.storage()
            .instance()
            .set(&DataKey::JurorPool, &jurors);
        if !env.storage().instance().has(&DataKey::ActiveJurors) {
            env.storage()
                .instance()
                .set(&DataKey::ActiveJurors, &Vec::<Address>::new(&env));
        }
        Ok(())
    }

    /// Return the list of available jurors in the candidate pool.
    pub fn get_juror_pool(env: Env) -> Vec<Address> {
        env.storage()
            .instance()
            .get(&DataKey::JurorPool)
            .or_else(|| env.storage().instance().get(&DataKey::ActiveJurors))
            .unwrap_or_else(|| Vec::new(&env))
    }

    /// Return the list of jurors currently assigned to active disputes.
    pub fn get_active_jurors(env: Env) -> Vec<Address> {
        env.storage()
            .instance()
            .get(&DataKey::ActiveJurors)
            .unwrap_or_else(|| Vec::new(&env))
    }

    /// File a dispute against an agent.
    ///
    /// Selects a pseudo-randomized subset of `JUROR_COUNT` jurors from `JurorPool`
    /// derived from `dispute_id`, `filed_at`, and the ledger sequence number.
    /// Updates `ActiveJurors` in instance storage to track jurors assigned to active disputes.
    pub fn file_dispute(
        env: Env,
        filer: Address,
        agent_id: Symbol,
        dispute_id: Symbol,
        bond_amount: i128,
    ) -> Result<(), Error> {
        require_not_paused(&env)?;
        filer.require_auth();

        if bond_amount < 0 {
            return Err(Error::InvalidBond);
        }

        let now = env.ledger().timestamp();

        // Check for duplicate
        let key = DataKey::Dispute(dispute_id.clone());
        if env.storage().persistent().has(&key) {
            return Err(Error::AlreadyExists);
        }

        let pool = Self::get_juror_pool(env.clone());
        if pool.is_empty() {
            return Err(Error::NoJurorsAvailable);
        }

        // Pseudo-random selection mechanism: seed = sha256(dispute_id || now || sequence)
        let mut available_pool = pool.clone();
        let mut selected_jurors = Vec::new(&env);
        let count = available_pool.len().min(JUROR_COUNT);

        let mut seed_preimage = Bytes::new(&env);
        seed_preimage.append(&dispute_id.clone().to_xdr(&env));
        seed_preimage.append(&now.to_xdr(&env));
        seed_preimage.append(&env.ledger().sequence().to_xdr(&env));
        let hash: BytesN<32> = env.crypto().sha256(&seed_preimage).into();
        let bytes = hash.to_array();

        for i in 0..count {
            let byte_val = bytes[i as usize] as u32;
            let idx = byte_val % available_pool.len();
            let juror = available_pool.get(idx).unwrap();
            selected_jurors.push_back(juror.clone());
            available_pool.remove(idx);
        }

        // Update ActiveJurors instance storage to reflect currently assigned active jurors
        let mut active_jurors = Self::get_active_jurors(env.clone());
        for i in 0..selected_jurors.len() {
            let j = selected_jurors.get(i).unwrap();
            if !active_jurors.contains(&j) {
                active_jurors.push_back(j);
            }
        }
        env.storage()
            .instance()
            .set(&DataKey::ActiveJurors, &active_jurors);

        let dispute = Dispute {
            dispute_id: dispute_id.clone(),
            filer: filer.clone(),
            agent_id: agent_id.clone(),
            status: DisputeStatus::Filed,
            filed_at: now,
            evidence_deadline: now + EVIDENCE_PHASE,
            voting_deadline: now + EVIDENCE_PHASE + VOTING_PHASE,
            appeal_deadline: now + EVIDENCE_PHASE + VOTING_PHASE + APPEAL_WINDOW,
            jurors: selected_jurors,
            appealed: false,
            resolution: None,
            bond_amount,
        };

        env.storage().persistent().set(&key, &dispute);

        env.events().publish(
            (symbol_short!("dispute"), symbol_short!("filed")),
            DisputeFiledEvent {
                dispute_id: dispute_id.clone(),
                filer,
                agent_id,
            },
        );

        Ok(())
    }

    /// Submit evidence to a dispute.
    pub fn submit_evidence(
        env: Env,
        dispute_id: Symbol,
        submitter: Address,
        evidence_hash: BytesN<32>,
    ) -> Result<(), Error> {
        require_not_paused(&env)?;
        submitter.require_auth();

        let key = DataKey::Dispute(dispute_id.clone());
        let mut dispute: Dispute = env
            .storage()
            .persistent()
            .get(&key)
            .ok_or(Error::NotFound)?;

        let now = env.ledger().timestamp();
        if now > dispute.evidence_deadline {
            return Err(Error::DisputeExpired);
        }

        let evidence_count_key = DataKey::Evidence(dispute_id.clone(), 0);
        let evidence_count: u32 = env
            .storage()
            .persistent()
            .get(&evidence_count_key)
            .unwrap_or(0);

        let evidence = Evidence {
            dispute_id: dispute_id.clone(),
            submitter: submitter.clone(),
            evidence_hash,
            submitted_at: now,
        };

        let ev_key = DataKey::Evidence(dispute_id.clone(), evidence_count);
        env.storage().persistent().set(&ev_key, &evidence);
        env.storage()
            .persistent()
            .set(&evidence_count_key, &(evidence_count + 1));

        // Move to evidence submission phase if still in filed status
        if dispute.status == DisputeStatus::Filed {
            dispute.status = DisputeStatus::EvidenceSubmission;
            env.storage().persistent().set(&key, &dispute);
        }

        env.events().publish(
            (symbol_short!("dispute"), symbol_short!("evidence")),
            EvidenceSubmittedEvent {
                dispute_id,
                submitter,
            },
        );

        Ok(())
    }

    /// Juror casts a vote on a dispute.
    pub fn cast_vote(
        env: Env,
        dispute_id: Symbol,
        juror: Address,
        side: VoteSide,
    ) -> Result<(), Error> {
        require_not_paused(&env)?;
        juror.require_auth();

        let key = DataKey::Dispute(dispute_id.clone());
        let mut dispute: Dispute = env
            .storage()
            .persistent()
            .get(&key)
            .ok_or(Error::NotFound)?;

        let now = env.ledger().timestamp();
        if now > dispute.voting_deadline {
            return Err(Error::DisputeExpired);
        }

        if dispute.status == DisputeStatus::Resolved {
            return Err(Error::DisputeAlreadyResolved);
        }

        // Verify juror is assigned
        if !dispute.jurors.contains(&juror) {
            return Err(Error::NotJuror);
        }

        // Check if already voted
        let vote_key = DataKey::JurorVote(dispute_id.clone(), juror.clone());
        if env.storage().persistent().has(&vote_key) {
            return Err(Error::JurorAlreadyVoted);
        }

        let vote = JurorVote {
            dispute_id: dispute_id.clone(),
            juror: juror.clone(),
            side: side.clone(),
            voted_at: now,
        };
        env.storage().persistent().set(&vote_key, &vote);

        // Move to voting phase if in evidence submission
        if dispute.status == DisputeStatus::EvidenceSubmission
            || dispute.status == DisputeStatus::Filed
        {
            dispute.status = DisputeStatus::Voting;
            env.storage().persistent().set(&key, &dispute);
        }

        env.events().publish(
            (symbol_short!("dispute"), symbol_short!("vote_cast")),
            VoteCastEvent {
                dispute_id: dispute_id.clone(),
                juror: juror.clone(),
                side: side.clone(),
                timestamp: now,
            },
        );

        Ok(())
    }

    /// Resolve a dispute after voting period ends (admin or automated).
    ///
    /// Tie-breaking behavior: If client_votes > agent_votes, resolution is 0 (Client wins).
    /// Otherwise (including when client_votes == agent_votes), resolution defaults to 1 (Agent wins).
    pub fn resolve_dispute(env: Env, dispute_id: Symbol) -> Result<(), Error> {
        require_not_paused(&env)?;

        let key = DataKey::Dispute(dispute_id.clone());
        let mut dispute: Dispute = env
            .storage()
            .persistent()
            .get(&key)
            .ok_or(Error::NotFound)?;

        if dispute.status == DisputeStatus::Resolved {
            return Err(Error::DisputeAlreadyResolved);
        }

        let now = env.ledger().timestamp();
        if now <= dispute.voting_deadline {
            return Err(Error::DisputeExpired);
        }

        // Count votes
        let mut client_votes = 0u32;
        let mut agent_votes = 0u32;

        for juror in dispute.jurors.iter() {
            let vote_key = DataKey::JurorVote(dispute_id.clone(), juror);
            if let Some(vote) = env.storage().persistent().get::<_, JurorVote>(&vote_key) {
                match vote.side {
                    VoteSide::Client => client_votes += 1,
                    VoteSide::Agent => agent_votes += 1,
                }
            }
        }

        let resolution = if client_votes > agent_votes { 0 } else { 1 };

        dispute.status = DisputeStatus::Resolved;
        dispute.resolution = Some(resolution);
        env.storage().persistent().set(&key, &dispute);

        // Update ActiveJurors to remove resolved dispute's jurors
        let active_jurors = Self::get_active_jurors(env.clone());
        let mut new_active = Vec::new(&env);
        for i in 0..active_jurors.len() {
            let j = active_jurors.get(i).unwrap();
            if !dispute.jurors.contains(&j) {
                new_active.push_back(j);
            }
        }
        env.storage()
            .instance()
            .set(&DataKey::ActiveJurors, &new_active);

        env.events().publish(
            (symbol_short!("dispute"), symbol_short!("resolved")),
            DisputeResolvedEvent {
                dispute_id,
                resolution,
                bond_amount: dispute.bond_amount,
            },
        );

        Ok(())
    }

    /// Appeal a resolved dispute (must be within appeal window).
    pub fn appeal_dispute(env: Env, dispute_id: Symbol, appellant: Address) -> Result<(), Error> {
        require_not_paused(&env)?;
        appellant.require_auth();

        let key = DataKey::Dispute(dispute_id.clone());
        let mut dispute: Dispute = env
            .storage()
            .persistent()
            .get(&key)
            .ok_or(Error::NotFound)?;

        if dispute.status != DisputeStatus::Resolved {
            return Err(Error::DisputeAlreadyResolved);
        }

        let now = env.ledger().timestamp();
        if now > dispute.appeal_deadline {
            return Err(Error::AppealWindowClosed);
        }

        if dispute.appealed {
            return Err(Error::AlreadyExists);
        }

        dispute.appealed = true;
        dispute.status = DisputeStatus::Appealed;
        env.storage().persistent().set(&key, &dispute);

        env.events().publish(
            (symbol_short!("dispute"), symbol_short!("appealed")),
            DisputeAppealedEvent {
                dispute_id,
                appellant,
            },
        );

        Ok(())
    }

    /// Get a dispute by ID.
    pub fn get_dispute(env: Env, dispute_id: Symbol) -> Option<Dispute> {
        env.storage()
            .persistent()
            .get(&DataKey::Dispute(dispute_id))
    }

    /// Get evidence count for a dispute.
    pub fn get_evidence_count(env: Env, dispute_id: Symbol) -> u32 {
        let count_key = DataKey::Evidence(dispute_id, 0);
        env.storage().persistent().get(&count_key).unwrap_or(0)
    }
}

#[cfg(test)]
mod test;

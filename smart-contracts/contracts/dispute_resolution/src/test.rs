//! Unit tests for the multi-phase dispute contract. These are updated with
//! the contract interface but intentionally not executed in this change.

extern crate std;

use super::*;
use soroban_sdk::{
    testutils::{Address as _, Ledger as _}, Address, BytesN, Env, String, Symbol,
};

struct Fixture {
    env: Env,
    client: DisputeResolutionContractClient<'static>,
    voters: [Address; 5],
    filer: Address,
    agent: Address,
    task_id: Symbol,
}

fn setup() -> Fixture {
    let env = Env::default();
    env.mock_all_auths();
    let contract_id = env.register(DisputeResolutionContract, ());
    let client = DisputeResolutionContractClient::new(&env, &contract_id);
    let admin = Address::generate(&env);
    client.initialize(&admin);

    let voters = [
        Address::generate(&env),
        Address::generate(&env),
        Address::generate(&env),
        Address::generate(&env),
        Address::generate(&env),
    ];
    let voter_vec = soroban_sdk::vec![
        &env,
        voters[0].clone(),
        voters[1].clone(),
        voters[2].clone(),
        voters[3].clone(),
        voters[4].clone(),
    ];
    client.set_voters(&voter_vec);
    for voter in voters.iter() {
        client.set_reputation(voter, &3);
    }

    let filer = Address::generate(&env);
    let agent = Address::generate(&env);
    let task_id = Symbol::new(&env, "task1");
    client.set_reputation(&filer, &10);
    client.set_reputation(&agent, &10);
    client.set_agent_bond(&agent, &100);
    client.set_task_escrow(&task_id, &1_000);

    Fixture {
        env,
        client,
        voters,
        filer,
        agent,
        task_id,
    }
}

fn file(fixture: &Fixture) {
    fixture.client.file_dispute(
        &fixture.task_id,
        &fixture.filer,
        &fixture.agent,
        &String::from_str(&fixture.env, "work was not delivered"),
    );
}

fn advance_to_voting(fixture: &Fixture) {
    fixture.env.ledger().with_mut(|ledger| {
        ledger.timestamp += EVIDENCE_PHASE;
    });
}

fn advance_to_resolution(fixture: &Fixture) {
    fixture.env.ledger().with_mut(|ledger| {
        ledger.timestamp += EVIDENCE_PHASE + VOTING_PHASE;
    });
}

#[test]
fn filing_validates_reason_and_stores_record() {
    let fixture = setup();
    let id = fixture.client.file_dispute(
        &fixture.task_id,
        &fixture.filer,
        &fixture.agent,
        &String::from_str(&fixture.env, "incorrect output"),
    );
    assert_eq!(id, fixture.task_id);
    let dispute = fixture.client.get_dispute(&id).unwrap();
    assert_eq!(dispute.status, DisputeStatus::EvidencePhase);
    assert_eq!(dispute.evidence_deadline, dispute.filed_at + EVIDENCE_PHASE);
    assert_eq!(dispute.voting_deadline, dispute.evidence_deadline + VOTING_PHASE);
}

#[test]
fn both_parties_can_submit_evidence_and_outsiders_cannot() {
    let fixture = setup();
    file(&fixture);
    let hash = BytesN::from_array(&fixture.env, &[7u8; 32]);
    fixture
        .client
        .submit_evidence(&fixture.task_id, &fixture.filer, &hash);
    fixture
        .client
        .submit_evidence(&fixture.task_id, &fixture.agent, &hash);
    assert_eq!(fixture.client.get_evidence_count(&fixture.task_id), 2);

    let outsider = Address::generate(&fixture.env);
    assert_eq!(
        fixture
            .client
            .try_submit_evidence(&fixture.task_id, &outsider, &hash),
        Err(Ok(Error::Unauthorized))
    );
}

#[test]
fn vote_requires_reputation_and_prevents_double_voting() {
    let fixture = setup();
    file(&fixture);
    let voter = fixture.voters[0].clone();
    assert_eq!(
        fixture
            .client
            .try_vote(&fixture.task_id, &voter, &VoteSide::SupportFiler),
        Err(Ok(Error::InvalidPhase))
    );
    advance_to_voting(&fixture);
    fixture.client.set_reputation(&voter, &2);
    assert_eq!(
        fixture
            .client
            .try_vote(&fixture.task_id, &voter, &VoteSide::SupportFiler),
        Err(Ok(Error::NotEligibleVoter))
    );
    fixture.client.set_reputation(&voter, &3);
    fixture
        .client
        .vote(&fixture.task_id, &voter, &VoteSide::SupportFiler);
    assert_eq!(
        fixture
            .client
            .try_vote(&fixture.task_id, &voter, &VoteSide::SupportAgent),
        Err(Ok(Error::AlreadyVoted))
    );
}

#[test]
fn support_filer_refunds_and_slashes_half_the_agent_bond() {
    let fixture = setup();
    file(&fixture);
    advance_to_voting(&fixture);
    for voter in fixture.voters.iter().take(3) {
        fixture
            .client
            .vote(&fixture.task_id, voter, &VoteSide::SupportFiler);
    }
    for voter in fixture.voters.iter().skip(3) {
        fixture
            .client
            .vote(&fixture.task_id, voter, &VoteSide::SupportAgent);
    }
    advance_to_resolution(&fixture);

    assert_eq!(
        fixture.client.resolve(&fixture.task_id),
        DisputeOutcome::SupportFiler
    );
    let dispute = fixture.client.get_dispute(&fixture.task_id).unwrap();
    assert_eq!(dispute.bond_slashed, 50);
    assert_eq!(fixture.client.get_agent_bond(&fixture.agent), 50);
    assert_eq!(dispute.filer_refund, 1_000);
    assert_eq!(dispute.agent_payment, 0);
    assert_eq!(dispute.status, DisputeStatus::Resolved);
}

#[test]
fn insufficient_votes_return_a_neutral_split_without_penalties() {
    let fixture = setup();
    file(&fixture);
    advance_to_voting(&fixture);
    fixture
        .client
        .vote(&fixture.task_id, &fixture.voters[0], &VoteSide::SupportFiler);
    advance_to_resolution(&fixture);

    assert_eq!(fixture.client.resolve(&fixture.task_id), DisputeOutcome::Tie);
    let dispute = fixture.client.get_dispute(&fixture.task_id).unwrap();
    assert_eq!(dispute.filer_refund, 500);
    assert_eq!(dispute.agent_payment, 500);
    assert_eq!(dispute.bond_slashed, 0);
    assert_eq!(fixture.client.get_agent_bond(&fixture.agent), 100);
}

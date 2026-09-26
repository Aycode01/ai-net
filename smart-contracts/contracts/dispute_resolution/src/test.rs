//! # Dispute Resolution Unit Tests

extern crate std;

use super::*;
use soroban_sdk::{
    testutils::{Address as _, Events, Ledger as _},
    Address, BytesN, Env, Symbol, Vec,
};

fn setup() -> (Env, DisputeResolutionContractClient<'static>) {
    let env = Env::default();
    env.mock_all_auths();
    let id = env.register(DisputeResolutionContract, ());
    let client = DisputeResolutionContractClient::new(&env, &id);
    (env, client)
}

fn setup_with_admin() -> (Env, DisputeResolutionContractClient<'static>, Address) {
    let env = Env::default();
    env.mock_all_auths();
    let id = env.register(DisputeResolutionContract, ());
    let client = DisputeResolutionContractClient::new(&env, &id);
    let admin = Address::generate(&env);
    client.initialize(&admin);
    (env, client, admin)
}

fn setup_with_jurors() -> (
    Env,
    DisputeResolutionContractClient<'static>,
    Address,
    Vec<'static, Address>,
) {
    let (env, client, admin) = setup_with_admin();
    let jurors = soroban_sdk::vec![
        &env,
        Address::generate(&env),
        Address::generate(&env),
        Address::generate(&env),
        Address::generate(&env),
        Address::generate(&env),
    ];
    client.set_jurors(&jurors);
    (env, client, admin, jurors)
}

#[test]
fn initialize_sets_admin() {
    let (env, client) = setup();
    let admin = Address::generate(&env);
    client.initialize(&admin);
    env.as_contract(&client.address, || {
        assert!(env.storage().instance().has(&DataKey::Admin));
    });
}

#[test]
fn file_dispute_success() {
    let (env, client, _admin, _jurors) = setup_with_jurors();
    let filer = Address::generate(&env);
    let dispute_id = Symbol::new(&env, "disp1");

    client.file_dispute(&filer, &Symbol::new(&env, "agent1"), &dispute_id, &100_000);
    let dispute = client.get_dispute(&dispute_id);
    assert!(dispute.is_some());
    let dispute = dispute.unwrap();
    assert_eq!(dispute.status, DisputeStatus::Filed);
    assert_eq!(dispute.agent_id, Symbol::new(&env, "agent1"));
    assert_eq!(dispute.bond_amount, 100_000);
}

#[test]
fn file_dispute_no_jurors_fails() {
    let (env, client) = setup();
    let admin = Address::generate(&env);
    client.initialize(&admin);
    let filer = Address::generate(&env);

    assert_eq!(
        client.try_file_dispute(
            &filer,
            &Symbol::new(&env, "agent1"),
            &Symbol::new(&env, "disp_bad"),
            &0
        ),
        Err(Ok(Error::NoJurorsAvailable))
    );
}

#[test]
fn file_dispute_invalid_bond_fails() {
    let (env, client, _admin, _jurors) = setup_with_jurors();
    let filer = Address::generate(&env);

    assert_eq!(
        client.try_file_dispute(
            &filer,
            &Symbol::new(&env, "agent1"),
            &Symbol::new(&env, "disp_bad_bond"),
            &-100
        ),
        Err(Ok(Error::InvalidBond))
    );
}

#[test]
fn submit_evidence_success() {
    let (env, client, _admin, _jurors) = setup_with_jurors();
    let filer = Address::generate(&env);
    let dispute_id = Symbol::new(&env, "disp1");
    client.file_dispute(&filer, &Symbol::new(&env, "agent1"), &dispute_id, &0);

    let mut arr = [0u8; 32];
    arr[0] = 42;
    let hash = BytesN::from_array(&env, &arr);

    client.submit_evidence(&dispute_id, &filer, &hash);

    assert_eq!(client.get_evidence_count(&dispute_id), 1);
}

#[test]
fn cast_vote_success_and_emits_event() {
    let (env, client, _admin, jurors) = setup_with_jurors();
    let filer = Address::generate(&env);
    let dispute_id = Symbol::new(&env, "disp1");
    client.file_dispute(&filer, &Symbol::new(&env, "agent1"), &dispute_id, &0);

    let dispute = client.get_dispute(&dispute_id).unwrap();
    let juror = dispute.jurors.get(0).unwrap();
    client.cast_vote(&dispute_id, &juror, &VoteSide::Client);

    let updated_dispute = client.get_dispute(&dispute_id).unwrap();
    assert_eq!(updated_dispute.status, DisputeStatus::Voting);

    let events = env.events().all();
    assert!(!events.is_empty());
}

#[test]
fn cast_vote_non_juror_fails() {
    let (env, client, _admin, _jurors) = setup_with_jurors();
    let filer = Address::generate(&env);
    let dispute_id = Symbol::new(&env, "disp1");
    client.file_dispute(&filer, &Symbol::new(&env, "agent1"), &dispute_id, &0);

    let outsider = Address::generate(&env);
    assert_eq!(
        client.try_cast_vote(&dispute_id, &outsider, &VoteSide::Client),
        Err(Ok(Error::NotJuror))
    );
}

#[test]
fn cast_vote_duplicate_fails() {
    let (env, client, _admin, jurors) = setup_with_jurors();
    let filer = Address::generate(&env);
    let dispute_id = Symbol::new(&env, "disp1");
    client.file_dispute(&filer, &Symbol::new(&env, "agent1"), &dispute_id, &0);

    let dispute = client.get_dispute(&dispute_id).unwrap();
    let juror = dispute.jurors.get(0).unwrap();
    client.cast_vote(&dispute_id, &juror, &VoteSide::Client);

    assert_eq!(
        client.try_cast_vote(&dispute_id, &juror, &VoteSide::Agent),
        Err(Ok(Error::JurorAlreadyVoted))
    );
}

#[test]
fn resolve_dispute_after_voting() {
    let (env, client, _admin, _jurors) = setup_with_jurors();

    let filer = Address::generate(&env);
    let dispute_id = Symbol::new(&env, "disp1");
    client.file_dispute(&filer, &Symbol::new(&env, "agent1"), &dispute_id, &0);

    let dispute = client.get_dispute(&dispute_id).unwrap();
    let assigned_jurors = dispute.jurors;

    // Cast votes: 3 for client, 2 for agent
    client.cast_vote(&dispute_id, &assigned_jurors.get(0).unwrap(), &VoteSide::Client);
    client.cast_vote(&dispute_id, &assigned_jurors.get(1).unwrap(), &VoteSide::Client);
    client.cast_vote(&dispute_id, &assigned_jurors.get(2).unwrap(), &VoteSide::Client);
    client.cast_vote(&dispute_id, &assigned_jurors.get(3).unwrap(), &VoteSide::Agent);
    client.cast_vote(&dispute_id, &assigned_jurors.get(4).unwrap(), &VoteSide::Agent);

    // Advance past voting deadline
    env.ledger().with_mut(|l| {
        l.timestamp += EVIDENCE_PHASE + VOTING_PHASE + 1;
    });

    client.resolve_dispute(&dispute_id);

    let dispute = client.get_dispute(&dispute_id).unwrap();
    assert_eq!(dispute.status, DisputeStatus::Resolved);
    assert_eq!(dispute.resolution, Some(0)); // Client wins
}

#[test]
fn tie_breaking_defaults_to_agent() {
    let (env, client, _admin, _jurors) = setup_with_jurors();

    let filer = Address::generate(&env);
    let dispute_id = Symbol::new(&env, "disp_tie");
    client.file_dispute(&filer, &Symbol::new(&env, "agent1"), &dispute_id, &0);

    let dispute = client.get_dispute(&dispute_id).unwrap();
    let assigned_jurors = dispute.jurors;

    // Cast equal votes: 2 for client, 2 for agent
    client.cast_vote(&dispute_id, &assigned_jurors.get(0).unwrap(), &VoteSide::Client);
    client.cast_vote(&dispute_id, &assigned_jurors.get(1).unwrap(), &VoteSide::Client);
    client.cast_vote(&dispute_id, &assigned_jurors.get(2).unwrap(), &VoteSide::Agent);
    client.cast_vote(&dispute_id, &assigned_jurors.get(3).unwrap(), &VoteSide::Agent);

    // Advance past voting deadline
    env.ledger().with_mut(|l| {
        l.timestamp += EVIDENCE_PHASE + VOTING_PHASE + 1;
    });

    client.resolve_dispute(&dispute_id);

    let dispute = client.get_dispute(&dispute_id).unwrap();
    assert_eq!(dispute.status, DisputeStatus::Resolved);
    assert_eq!(dispute.resolution, Some(1)); // Tie defaults to Agent (1)
}

#[test]
fn randomized_juror_selection_divergence() {
    let (env, client, admin) = setup_with_admin();
    let pool = soroban_sdk::vec![
        &env,
        Address::generate(&env),
        Address::generate(&env),
        Address::generate(&env),
        Address::generate(&env),
        Address::generate(&env),
        Address::generate(&env),
        Address::generate(&env),
        Address::generate(&env),
        Address::generate(&env),
        Address::generate(&env),
    ];
    client.set_jurors(&pool);

    let filer = Address::generate(&env);
    let d1 = Symbol::new(&env, "disp1");
    let d2 = Symbol::new(&env, "disp2");

    client.file_dispute(&filer, &Symbol::new(&env, "agent1"), &d1, &0);
    client.file_dispute(&filer, &Symbol::new(&env, "agent1"), &d2, &0);

    let disp1 = client.get_dispute(&d1).unwrap();
    let disp2 = client.get_dispute(&d2).unwrap();

    assert_eq!(disp1.jurors.len(), 5);
    assert_eq!(disp2.jurors.len(), 5);
    assert_ne!(disp1.jurors, disp2.jurors);
}

#[test]
fn active_jurors_tracked_and_cleared_on_resolve() {
    let (env, client, _admin, _jurors) = setup_with_jurors();
    let filer = Address::generate(&env);
    let dispute_id = Symbol::new(&env, "disp_active");

    client.file_dispute(&filer, &Symbol::new(&env, "agent1"), &dispute_id, &0);
    let active_before = client.get_active_jurors();
    assert_eq!(active_before.len(), 5);

    // Fast-forward and resolve
    env.ledger().with_mut(|l| {
        l.timestamp += EVIDENCE_PHASE + VOTING_PHASE + 1;
    });
    client.resolve_dispute(&dispute_id);

    let active_after = client.get_active_jurors();
    assert_eq!(active_after.len(), 0);
}

#[test]
fn appeal_dispute_success() {
    let (env, client, _admin, _jurors) = setup_with_jurors();

    let filer = Address::generate(&env);
    let dispute_id = Symbol::new(&env, "disp1");
    client.file_dispute(&filer, &Symbol::new(&env, "agent1"), &dispute_id, &0);

    let dispute = client.get_dispute(&dispute_id).unwrap();
    client.cast_vote(&dispute_id, &dispute.jurors.get(0).unwrap(), &VoteSide::Agent);

    // Advance past voting deadline
    env.ledger().with_mut(|l| {
        l.timestamp += EVIDENCE_PHASE + VOTING_PHASE + 1;
    });

    client.resolve_dispute(&dispute_id);

    let appellant = Address::generate(&env);
    client.appeal_dispute(&dispute_id, &appellant);

    let dispute = client.get_dispute(&dispute_id).unwrap();
    assert_eq!(dispute.status, DisputeStatus::Appealed);
    assert!(dispute.appealed);
}

#[test]
fn appeal_after_window_fails() {
    let (env, client, _admin, _jurors) = setup_with_jurors();

    let filer = Address::generate(&env);
    let dispute_id = Symbol::new(&env, "disp1");
    client.file_dispute(&filer, &Symbol::new(&env, "agent1"), &dispute_id, &0);

    let dispute = client.get_dispute(&dispute_id).unwrap();
    client.cast_vote(&dispute_id, &dispute.jurors.get(0).unwrap(), &VoteSide::Agent);

    // Advance past appeal deadline
    env.ledger().with_mut(|l| {
        l.timestamp += DISPUTE_WINDOW + 100;
    });

    // resolve_dispute should work since we're past voting deadline
    let _ = client.try_resolve_dispute(&dispute_id);

    let appellant = Address::generate(&env);
    let res = client.try_appeal_dispute(&dispute_id, &appellant);
    assert!(res.is_err());
}

#[test]
fn pause_blocks_filing() {
    let (env, client, _admin) = setup_with_admin();
    client.pause();

    let filer = Address::generate(&env);
    assert_eq!(
        client.try_file_dispute(
            &filer,
            &Symbol::new(&env, "agent1"),
            &Symbol::new(&env, "disp_pause"),
            &0
        ),
        Err(Ok(Error::ContractPaused))
    );
}

#[test]
fn unpause_allows_filing() {
    let (env, client, _admin, _jurors) = setup_with_jurors();
    client.pause();
    client.unpause();

    let filer = Address::generate(&env);
    client.file_dispute(
        &filer,
        &Symbol::new(&env, "agent1"),
        &Symbol::new(&env, "disp_unpause"),
        &0,
    );
    assert!(client
        .get_dispute(&Symbol::new(&env, "disp_unpause"))
        .is_some());
}

#[test]
fn is_paused_reflects_state() {
    let (_env, client, _admin) = setup_with_admin();
    assert!(!client.is_paused());
    client.pause();
    assert!(client.is_paused());
    client.unpause();
    assert!(!client.is_paused());
}

#[test]
fn pause_blocks_set_jurors() {
    let (env, client, _admin) = setup_with_admin();
    client.pause();

    let jurors = soroban_sdk::vec![&env, Address::generate(&env)];
    assert_eq!(
        client.try_set_jurors(&jurors),
        Err(Ok(Error::ContractPaused))
    );
}

#[test]
fn get_dispute_still_works_when_paused() {
    let (env, client, _admin, _jurors) = setup_with_jurors();
    let filer = Address::generate(&env);
    client.file_dispute(
        &filer,
        &Symbol::new(&env, "agent1"),
        &Symbol::new(&env, "disp_read"),
        &0,
    );

    client.pause();

    // Reads should still work when paused.
    assert!(client
        .get_dispute(&Symbol::new(&env, "disp_read"))
        .is_some());
}

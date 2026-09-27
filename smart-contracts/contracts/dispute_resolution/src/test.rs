//! # Dispute Resolution Unit Tests

extern crate std;

use super::*;
use soroban_sdk::{
    testutils::{Address as _, Events as _, Ledger as _},
    Address, BytesN, Env, IntoVal, Symbol, TryFromVal, TryIntoVal, Val, Vec,
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
    Vec<Address>,
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

    client.file_dispute(&filer, &Symbol::new(&env, "agent1"), &dispute_id);
    let dispute = client.get_dispute(&dispute_id);
    assert!(dispute.is_some());
    let dispute = dispute.unwrap();
    assert_eq!(dispute.status, DisputeStatus::Filed);
    assert_eq!(dispute.agent_id, Symbol::new(&env, "agent1"));
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
            &Symbol::new(&env, "disp_bad")
        ),
        Err(Ok(Error::NoJurorsAvailable))
    );
}

#[test]
fn submit_evidence_success() {
    let (env, client, _admin, _jurors) = setup_with_jurors();
    let filer = Address::generate(&env);
    let dispute_id = Symbol::new(&env, "disp1");
    client.file_dispute(&filer, &Symbol::new(&env, "agent1"), &dispute_id);

    let mut arr = [0u8; 32];
    arr[0] = 42;
    let hash = BytesN::from_array(&env, &arr);

    client.submit_evidence(&dispute_id, &filer, &hash);

    assert_eq!(client.get_evidence_count(&dispute_id), 1);
}

#[test]
fn cast_vote_success() {
    let (env, client, _admin, jurors) = setup_with_jurors();
    let filer = Address::generate(&env);
    let dispute_id = Symbol::new(&env, "disp1");
    client.file_dispute(&filer, &Symbol::new(&env, "agent1"), &dispute_id);

    let juror = jurors.get(0).unwrap();
    client.cast_vote(&dispute_id, &juror, &VoteSide::Client);

    let dispute = client.get_dispute(&dispute_id).unwrap();
    assert_eq!(dispute.status, DisputeStatus::Voting);
}

#[test]
fn cast_vote_non_juror_fails() {
    let (env, client, _admin, _jurors) = setup_with_jurors();
    let filer = Address::generate(&env);
    let dispute_id = Symbol::new(&env, "disp1");
    client.file_dispute(&filer, &Symbol::new(&env, "agent1"), &dispute_id);

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
    client.file_dispute(&filer, &Symbol::new(&env, "agent1"), &dispute_id);

    let juror = jurors.get(0).unwrap();
    client.cast_vote(&dispute_id, &juror, &VoteSide::Client);

    assert_eq!(
        client.try_cast_vote(&dispute_id, &juror, &VoteSide::Agent),
        Err(Ok(Error::JurorAlreadyVoted))
    );
}

#[test]
fn resolve_dispute_after_voting() {
    let (env, client, _admin, jurors) = setup_with_jurors();

    let filer = Address::generate(&env);
    let dispute_id = Symbol::new(&env, "disp1");
    client.file_dispute(&filer, &Symbol::new(&env, "agent1"), &dispute_id);

    // Cast votes: 3 for client, 2 for agent
    client.cast_vote(&dispute_id, &jurors.get(0).unwrap(), &VoteSide::Client);
    client.cast_vote(&dispute_id, &jurors.get(1).unwrap(), &VoteSide::Client);
    client.cast_vote(&dispute_id, &jurors.get(2).unwrap(), &VoteSide::Client);
    client.cast_vote(&dispute_id, &jurors.get(3).unwrap(), &VoteSide::Agent);
    client.cast_vote(&dispute_id, &jurors.get(4).unwrap(), &VoteSide::Agent);

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
fn appeal_dispute_success() {
    let (env, client, _admin, jurors) = setup_with_jurors();

    let filer = Address::generate(&env);
    let dispute_id = Symbol::new(&env, "disp1");
    client.file_dispute(&filer, &Symbol::new(&env, "agent1"), &dispute_id);

    client.cast_vote(&dispute_id, &jurors.get(0).unwrap(), &VoteSide::Agent);

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
    let (env, client, _admin, jurors) = setup_with_jurors();

    let filer = Address::generate(&env);
    let dispute_id = Symbol::new(&env, "disp1");
    client.file_dispute(&filer, &Symbol::new(&env, "agent1"), &dispute_id);

    client.cast_vote(&dispute_id, &jurors.get(0).unwrap(), &VoteSide::Agent);

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
            &Symbol::new(&env, "disp_pause")
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
    );

    client.pause();

    // Reads should still work when paused.
    assert!(client
        .get_dispute(&Symbol::new(&env, "disp_read"))
        .is_some());
}

// ── Event emission coverage (issue #486) ────────────────────────────────────

/// Decode the data payload of every event from the latest invocation.
/// `env.events().all()` reflects only the most recent contract invocation,
/// so callers must query immediately after the mutating call.
fn last_events<T: TryFromVal<Env, Val>>(env: &Env) -> std::vec::Vec<T> {
    let events = env.events().all();
    let mut out = std::vec::Vec::new();
    for idx in 0..events.len() {
        let (_, _topics, data) = events.get(idx).unwrap();
        out.push(T::try_from_val(env, &data).unwrap());
    }
    out
}

#[test]
fn initialize_emits_init_event() {
    let (env, client) = setup();
    let admin = Address::generate(&env);
    client.initialize(&admin);

    let events = env.events().all();
    assert_eq!(events.len(), 1);
    let (_, topics, _) = events.get(0).unwrap();
    let t0: Symbol = topics.get(0).unwrap().try_into_val(&env).unwrap();
    let t1: Symbol = topics.get(1).unwrap().try_into_val(&env).unwrap();
    assert_eq!(t0, symbol_short!("dispute"));
    assert_eq!(t1, symbol_short!("init"));
}

#[test]
fn pause_and_unpause_emit_events_after_write() {
    let (env, client, _admin) = setup_with_admin();

    client.pause();
    let events = env.events().all();
    assert_eq!(events.len(), 1);
    let t1: Symbol = events
        .get(0)
        .unwrap()
        .1
        .get(1)
        .unwrap()
        .try_into_val(&env)
        .unwrap();
    assert_eq!(t1, symbol_short!("paused"));

    client.unpause();
    let events = env.events().all();
    assert_eq!(events.len(), 1);
    let t1: Symbol = events
        .get(0)
        .unwrap()
        .1
        .get(1)
        .unwrap()
        .try_into_val(&env)
        .unwrap();
    assert_eq!(t1, symbol_short!("unpaused"));
}

#[test]
fn set_jurors_emits_event_after_write() {
    let (env, client, _admin) = setup_with_admin();
    let jurors = soroban_sdk::vec![&env, Address::generate(&env), Address::generate(&env),];

    client.set_jurors(&jurors);

    // Query immediately: the pool must already be stored when the event is
    // observed.
    let payload: JurorsSetEvent = last_events(&env).remove(0);
    assert_eq!(payload.jurors.len(), 2);
    assert_eq!(payload.jurors.get(0), jurors.get(0));
    assert!(env.as_contract(&client.address, || {
        env.storage().instance().has(&DataKey::ActiveJurors)
    }));
}

#[test]
fn file_dispute_emits_event_after_write() {
    let (env, client, _admin, _jurors) = setup_with_jurors();
    let filer = Address::generate(&env);
    let dispute_id = Symbol::new(&env, "disp_ev");

    client.file_dispute(&filer, &Symbol::new(&env, "agent1"), &dispute_id);

    // Query immediately: the dispute must already be stored.
    let payload: DisputeFiledEvent = last_events(&env).remove(0);
    assert_eq!(payload.dispute_id, dispute_id);
    assert_eq!(payload.filer, filer);
    assert_eq!(payload.agent_id, Symbol::new(&env, "agent1"));
    assert!(client.get_dispute(&dispute_id).is_some());
}

#[test]
fn submit_evidence_emits_event_with_context_after_write() {
    let (env, client, _admin, _jurors) = setup_with_jurors();
    let filer = Address::generate(&env);
    let dispute_id = Symbol::new(&env, "disp_ev");
    client.file_dispute(&filer, &Symbol::new(&env, "agent1"), &dispute_id);

    let mut arr = [0u8; 32];
    arr[0] = 7;
    let hash = BytesN::from_array(&env, &arr);

    client.submit_evidence(&dispute_id, &filer, &hash);

    let payload: EvidenceSubmittedEvent = last_events(&env).remove(0);
    assert_eq!(payload.dispute_id, dispute_id);
    assert_eq!(payload.submitter, filer);
    assert_eq!(payload.evidence_hash, hash);
    assert_eq!(payload.evidence_index, 0);
    assert_eq!(client.get_evidence_count(&dispute_id), 1);
}

#[test]
fn cast_vote_emits_event_after_write() {
    let (env, client, _admin, jurors) = setup_with_jurors();
    let filer = Address::generate(&env);
    let dispute_id = Symbol::new(&env, "disp_ev");
    client.file_dispute(&filer, &Symbol::new(&env, "agent1"), &dispute_id);

    let juror = jurors.get(0).unwrap();
    client.cast_vote(&dispute_id, &juror, &VoteSide::Client);

    let payload: VoteCastEvent = last_events(&env).remove(0);
    assert_eq!(payload.dispute_id, dispute_id);
    assert_eq!(payload.juror, juror);
    assert_eq!(payload.side, VoteSide::Client);
}

#[test]
fn resolve_dispute_emits_event_after_write() {
    let (env, client, _admin, jurors) = setup_with_jurors();
    let filer = Address::generate(&env);
    let dispute_id = Symbol::new(&env, "disp_ev");
    client.file_dispute(&filer, &Symbol::new(&env, "agent1"), &dispute_id);

    client.cast_vote(&dispute_id, &jurors.get(0).unwrap(), &VoteSide::Client);
    client.cast_vote(&dispute_id, &jurors.get(1).unwrap(), &VoteSide::Client);
    client.cast_vote(&dispute_id, &jurors.get(2).unwrap(), &VoteSide::Agent);
    client.cast_vote(&dispute_id, &jurors.get(3).unwrap(), &VoteSide::Agent);
    client.cast_vote(&dispute_id, &jurors.get(4).unwrap(), &VoteSide::Agent);

    env.ledger().with_mut(|l| {
        l.timestamp += EVIDENCE_PHASE + VOTING_PHASE + 1;
    });

    client.resolve_dispute(&dispute_id);

    let payload: DisputeResolvedEvent = last_events(&env).remove(0);
    assert_eq!(payload.dispute_id, dispute_id);
    assert_eq!(payload.resolution, 1); // agent wins 3-2
    assert_eq!(payload.bond_amount, 0);
}

#[test]
fn appeal_dispute_emits_event_after_write() {
    let (env, client, _admin, jurors) = setup_with_jurors();
    let filer = Address::generate(&env);
    let dispute_id = Symbol::new(&env, "disp_ev");
    client.file_dispute(&filer, &Symbol::new(&env, "agent1"), &dispute_id);

    client.cast_vote(&dispute_id, &jurors.get(0).unwrap(), &VoteSide::Agent);
    env.ledger().with_mut(|l| {
        l.timestamp += EVIDENCE_PHASE + VOTING_PHASE + 1;
    });
    client.resolve_dispute(&dispute_id);

    let appellant = Address::generate(&env);
    client.appeal_dispute(&dispute_id, &appellant);

    let payload: DisputeAppealedEvent = last_events(&env).remove(0);
    assert_eq!(payload.dispute_id, dispute_id);
    assert_eq!(payload.appellant, appellant);
}

// ── Event payload roundtrips (issue #486) ───────────────────────────────────

fn assert_roundtrip<T>(env: &Env, original: T)
where
    T: Clone + IntoVal<Env, Val> + TryFromVal<Env, Val> + PartialEq + std::fmt::Debug,
{
    let val: Val = original.clone().into_val(env);
    let decoded: T = val.try_into_val(env).unwrap();
    assert_eq!(
        original, decoded,
        "event payload failed serialize/deserialize roundtrip"
    );
}

#[test]
fn dispute_event_payloads_roundtrip() {
    let env = Env::default();
    let filer = Address::generate(&env);
    let dispute_id = Symbol::new(&env, "rt");
    let mut arr = [0u8; 32];
    arr[0] = 9;
    let hash = BytesN::from_array(&env, &arr);
    let jurors = soroban_sdk::vec![&env, Address::generate(&env)];

    assert_roundtrip(
        &env,
        DisputeFiledEvent {
            dispute_id: dispute_id.clone(),
            filer: filer.clone(),
            agent_id: Symbol::new(&env, "agent1"),
        },
    );
    assert_roundtrip(
        &env,
        EvidenceSubmittedEvent {
            dispute_id: dispute_id.clone(),
            submitter: filer.clone(),
            evidence_hash: hash,
            submitted_at: 42,
            evidence_index: 1,
        },
    );
    assert_roundtrip(&env, JurorsSetEvent { jurors, set_at: 43 });
    assert_roundtrip(
        &env,
        VoteCastEvent {
            dispute_id: dispute_id.clone(),
            juror: filer.clone(),
            side: VoteSide::Agent,
        },
    );
    assert_roundtrip(
        &env,
        DisputeResolvedEvent {
            dispute_id: dispute_id.clone(),
            resolution: 1,
            bond_amount: 500,
        },
    );
    assert_roundtrip(
        &env,
        DisputeAppealedEvent {
            dispute_id,
            appellant: filer,
        },
    );
}

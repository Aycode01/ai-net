//! Unit tests for the multi-phase dispute contract.

extern crate std;

use super::*;
use agent_registry::{AgentRecord, AgentRegistryContract, AgentRegistryContractClient};
use soroban_sdk::{
    testutils::{Address as _, Ledger as _},
    Address, BytesN, Env, Map, String, Symbol,
};

struct Fixture {
    env: Env,
    client: DisputeResolutionContractClient<'static>,
    registry: AgentRegistryContractClient<'static>,
    registry_agent_id: Symbol,
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
    let registry_id = env.register(AgentRegistryContract, ());
    let registry = AgentRegistryContractClient::new(&env, &registry_id);
    let registry_agent_id = Symbol::new(&env, "registry_agent");
    registry.initialize(&admin);
    registry.set_min_bond(&100);
    registry.set_dispute_resolver(&contract_id);
    registry.register_agent(&AgentRecord {
        id: registry_agent_id.clone(),
        capability: Symbol::new(&env, "research"),
        price_stroops: 1,
        endpoint: String::from_str(&env, "https://agent.example"),
        owner: agent.clone(),
        metadata: Map::new(&env),
        bond_amount: 100,
    });
    client.set_agent_registry(&registry_id);
    client.set_agent_registry_id(&agent, &registry_agent_id);
    client.set_agent_bond(&agent, &100);
    client.set_task_escrow(&task_id, &1_000);

    Fixture {
        env,
        client,
        registry,
        registry_agent_id,
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

fn advance_to_appeal_finalization(fixture: &Fixture) {
    fixture.env.ledger().with_mut(|ledger| {
        ledger.timestamp += APPEAL_PHASE + 1;
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
    let proposed = fixture.client.get_dispute(&fixture.task_id).unwrap();
    assert_eq!(proposed.status, DisputeStatus::AppealPending);
    assert!(proposed.appeal_deadline.is_some());
    assert!(!proposed.appealed);
    assert_eq!(proposed.bond_slashed, 50);
    assert_eq!(fixture.client.get_agent_bond(&fixture.agent), 100);
    assert_eq!(
        fixture.client.try_finalize_dispute(&fixture.task_id),
        Err(Ok(Error::InvalidPhase))
    );
    assert_eq!(
        fixture.client.try_resolve(&fixture.task_id),
        Err(Ok(Error::InvalidPhase))
    );
    advance_to_appeal_finalization(&fixture);
    fixture.client.finalize_dispute(&fixture.task_id);
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
    advance_to_appeal_finalization(&fixture);
    fixture.client.finalize_dispute(&fixture.task_id);
    let dispute = fixture.client.get_dispute(&fixture.task_id).unwrap();
    assert_eq!(dispute.filer_refund, 500);
    assert_eq!(dispute.agent_payment, 500);
    assert_eq!(dispute.bond_slashed, 0);
    assert_eq!(fixture.client.get_agent_bond(&fixture.agent), 100);
}

#[test]
fn appeal_reopens_voting_and_reverses_provisional_slash() {
    let fixture = setup();
    file(&fixture);
    assert_eq!(
        fixture
            .registry
            .try_initiate_bond_return(&fixture.registry_agent_id),
        Err(Ok(agent_registry::Error::DisputePending))
    );
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
    assert_eq!(
        fixture.client.try_appeal_dispute(&fixture.task_id, &fixture.filer),
        Err(Ok(Error::Unauthorized))
    );
    fixture
        .client
        .appeal_dispute(&fixture.task_id, &fixture.agent);

    for voter in fixture.voters.iter().take(3) {
        fixture
            .client
            .vote(&fixture.task_id, voter, &VoteSide::SupportAgent);
    }
    for voter in fixture.voters.iter().skip(3) {
        fixture
            .client
            .vote(&fixture.task_id, voter, &VoteSide::SupportFiler);
    }
    fixture.env.ledger().with_mut(|ledger| {
        ledger.timestamp += VOTING_PHASE;
    });
    assert_eq!(
        fixture.client.resolve(&fixture.task_id),
        DisputeOutcome::SupportAgent
    );
    let dispute = fixture.client.get_dispute(&fixture.task_id).unwrap();
    assert_eq!(dispute.status, DisputeStatus::Resolved);
    assert_eq!(dispute.bond_slashed, 0);
    assert_eq!(fixture.client.get_agent_bond(&fixture.agent), 100);
    assert_eq!(dispute.agent_payment, 1_000);
    fixture
        .registry
        .initiate_bond_return(&fixture.registry_agent_id);
}

#[test]
fn finalized_verified_dispute_slashes_registry_bond_with_reason() {
    let fixture = setup();
    file(&fixture);
    advance_to_voting(&fixture);
    for voter in fixture.voters.iter().take(3) {
        fixture
            .client
            .vote(&fixture.task_id, voter, &VoteSide::SupportFiler);
    }
    advance_to_resolution(&fixture);
    fixture.client.resolve(&fixture.task_id);
    advance_to_appeal_finalization(&fixture);
    fixture.client.finalize_dispute(&fixture.task_id);

    let bond = fixture
        .registry
        .get_bond(&fixture.registry_agent_id)
        .unwrap();
    assert_eq!(bond.amount_stroops, 50);
    assert_eq!(bond.status, agent_registry::bond::BondStatus::Active);
}

#[test]
fn negative_auth_set_admin() {
    let (env, client, _admin) = setup_with_admin();
    let intruder = Address::generate(&env);
    env.mock_auths(&[]);
    assert!(client.try_set_admin(&intruder).is_err());
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

#![cfg(test)]

//! Tests for the append-only task version history and the per-creator task
//! index: `get_history`, `get_task_creator` and `get_tasks_by_creator`.
//!
//! The lifecycle, pause and oracle tests live in the inline `test` module in
//! `lib.rs`; this file covers only the version-history surface.

use super::*;
use soroban_sdk::{
    testutils::{Address as _, Ledger},
    Address, Bytes, BytesN, Env, Vec,
};

struct Fixture {
    env: Env,
    client: TaskStoreContractClient<'static>,
    submitter: Address,
    agent: Address,
    prompt_hash: BytesN<32>,
    dag: Bytes,
}

fn fixture() -> Fixture {
    let env = Env::default();
    env.mock_all_auths();
    env.ledger().with_mut(|ledger| {
        ledger.timestamp = 1_700_000_000;
        ledger.sequence_number = 100;
    });
    let contract_id = env.register(TaskStoreContract, ());
    let client = TaskStoreContractClient::new(&env, &contract_id);
    let admin = Address::generate(&env);
    client.initialize(&admin);

    Fixture {
        submitter: Address::generate(&env),
        agent: Address::generate(&env),
        prompt_hash: BytesN::from_array(&env, &[2; 32]),
        dag: Bytes::from_slice(&env, &[0x78, 0x9c, 0x03, 0x00]),
        env,
        client,
    }
}

fn task_id(fixture: &Fixture, seed: u8) -> BytesN<32> {
    BytesN::from_array(&fixture.env, &[seed; 32])
}

fn store(fixture: &Fixture, id: &BytesN<32>, submitter: &Address) {
    let agents = Vec::from_array(&fixture.env, [fixture.agent.clone()]);
    fixture.client.store_task_metadata(
        submitter,
        id,
        &fixture.prompt_hash,
        &agents,
        &fixture.dag,
        &1u32,
        &None,
    );
}

fn contains(ids: &Vec<BytesN<32>>, id: &BytesN<32>) -> bool {
    ids.iter().any(|candidate| &candidate == id)
}

// ── Creation seeds the history ────────────────────────────────────────────────

#[test]
fn creating_a_task_seeds_a_single_pending_version_record() {
    let f = fixture();
    let id = task_id(&f, 1);
    store(&f, &id, &f.submitter);

    let history = f.client.get_history(&id);
    assert_eq!(history.len(), 1);

    let record = history.get(0).unwrap();
    assert_eq!(record.seq, 1);
    assert_eq!(record.status, TaskStatus::Pending);
    assert_eq!(record.updater, f.submitter);
    assert_eq!(record.timestamp, f.env.ledger().timestamp());
    assert_eq!(record.ledger_sequence, f.env.ledger().sequence());
}

#[test]
fn get_history_rejects_an_unknown_task() {
    let f = fixture();
    assert_eq!(
        f.client.try_get_history(&task_id(&f, 7)),
        Err(Ok(Error::NotFound))
    );
}

#[test]
fn get_history_rejects_an_expired_task() {
    let f = fixture();
    let id = task_id(&f, 1);
    store(&f, &id, &f.submitter);

    f.env.ledger().with_mut(|ledger| {
        ledger.timestamp += SECONDS_PER_DAY;
    });

    assert_eq!(f.client.try_get_history(&id), Err(Ok(Error::Expired)));
}

// ── Every accepted transition appends a record ────────────────────────────────

#[test]
fn each_status_update_appends_one_version_record() {
    let f = fixture();
    let id = task_id(&f, 1);
    store(&f, &id, &f.submitter);

    f.client
        .update_task_status(&id, &f.agent, &TaskStatus::Running);
    f.client
        .update_task_status(&id, &f.agent, &TaskStatus::Completed);

    let history = f.client.get_history(&id);
    assert_eq!(history.len(), 3);

    let statuses = [
        TaskStatus::Pending,
        TaskStatus::Running,
        TaskStatus::Completed,
    ];
    for (index, status) in statuses.iter().enumerate() {
        let record = history.get(index as u32).unwrap();
        assert_eq!(record.seq, index as u32 + 1);
        assert_eq!(record.status, *status);
    }

    // The updater is the agent for transitions and the submitter for creation.
    assert_eq!(history.get(0).unwrap().updater, f.submitter);
    assert_eq!(history.get(1).unwrap().updater, f.agent);
    assert_eq!(history.get(2).unwrap().updater, f.agent);
}

#[test]
fn version_records_are_ordered_by_sequence_and_ledger() {
    let f = fixture();
    let id = task_id(&f, 1);
    store(&f, &id, &f.submitter);

    f.env.ledger().with_mut(|ledger| {
        ledger.sequence_number += 5;
    });
    f.client
        .update_task_status(&id, &f.agent, &TaskStatus::Running);

    f.env.ledger().with_mut(|ledger| {
        ledger.sequence_number += 5;
    });
    f.client
        .update_task_status(&id, &f.agent, &TaskStatus::Failed);

    let history = f.client.get_history(&id);
    assert_eq!(history.len(), 3);

    let mut previous_seq = 0;
    let mut previous_ledger = 0;
    for index in 0..history.len() {
        let record = history.get(index).unwrap();
        assert!(record.seq > previous_seq);
        assert!(record.ledger_sequence > previous_ledger);
        previous_seq = record.seq;
        previous_ledger = record.ledger_sequence;
    }
}

#[test]
fn a_terminal_transition_still_appends_a_record() {
    let f = fixture();
    let id = task_id(&f, 1);
    store(&f, &id, &f.submitter);

    // Pending -> Failed is a valid terminal transition.
    f.client
        .update_task_status(&id, &f.agent, &TaskStatus::Failed);

    let history = f.client.get_history(&id);
    assert_eq!(history.len(), 2);
    assert_eq!(history.get(1).unwrap().status, TaskStatus::Failed);
}

// ── Rejected calls leave no trace ─────────────────────────────────────────────

#[test]
fn a_rejected_transition_appends_no_record() {
    let f = fixture();
    let id = task_id(&f, 1);
    store(&f, &id, &f.submitter);

    // Pending -> Completed is not a legal transition.
    let _ = f
        .client
        .try_update_task_status(&id, &f.agent, &TaskStatus::Completed);

    assert_eq!(f.client.get_history(&id).len(), 1);
}

#[test]
fn an_unauthorized_agent_appends_no_record() {
    let f = fixture();
    let id = task_id(&f, 1);
    store(&f, &id, &f.submitter);
    let stranger = Address::generate(&f.env);

    let result = f
        .client
        .try_update_task_status(&id, &stranger, &TaskStatus::Running);
    assert_eq!(result, Err(Ok(Error::NotAssignedAgent)));

    assert_eq!(f.client.get_history(&id).len(), 1);
}

// ── Creator attribution ───────────────────────────────────────────────────────

#[test]
fn get_task_creator_returns_the_submitter() {
    let f = fixture();
    let id = task_id(&f, 1);
    store(&f, &id, &f.submitter);

    assert_eq!(f.client.get_task_creator(&id), Some(f.submitter.clone()));
}

#[test]
fn get_task_creator_is_none_for_an_unknown_task() {
    let f = fixture();
    assert_eq!(f.client.get_task_creator(&task_id(&f, 9)), None);
}

// ── Per-creator task index ────────────────────────────────────────────────────

#[test]
fn get_tasks_by_creator_lists_tasks_oldest_first() {
    let f = fixture();
    for seed in 1u8..=3 {
        store(&f, &task_id(&f, seed), &f.submitter);
    }

    let page = f
        .client
        .get_tasks_by_creator(&f.submitter, &0, &MAX_TASKS_PAGE_SIZE);

    assert_eq!(page.total, 3);
    assert_eq!(page.next_cursor, None);
    assert_eq!(page.task_ids.len(), 3);
    for seed in 1u8..=3 {
        assert!(contains(&page.task_ids, &task_id(&f, seed)));
    }
    // Newest task last.
    assert_eq!(page.task_ids.get(2), Some(task_id(&f, 3)));
}

#[test]
fn get_tasks_by_creator_paginates_with_a_cursor() {
    let f = fixture();
    for seed in 1u8..=3 {
        store(&f, &task_id(&f, seed), &f.submitter);
    }

    let first = f.client.get_tasks_by_creator(&f.submitter, &0, &2);
    assert_eq!(first.total, 3);
    assert_eq!(first.task_ids.len(), 2);
    assert_eq!(first.next_cursor, Some(2));
    assert_eq!(first.task_ids.get(0), Some(task_id(&f, 1)));

    let cursor = first.next_cursor.unwrap();
    let second = f.client.get_tasks_by_creator(&f.submitter, &cursor, &2);
    assert_eq!(second.task_ids.len(), 1);
    assert_eq!(second.task_ids.get(0), Some(task_id(&f, 3)));
    assert_eq!(second.next_cursor, None);
}

#[test]
fn get_tasks_by_creator_clamps_an_oversized_limit() {
    let f = fixture();
    let count = MAX_TASKS_PAGE_SIZE + 1;
    for i in 0..count {
        store(&f, &task_id(&f, (i + 1) as u8), &f.submitter);
    }

    let page = f.client.get_tasks_by_creator(&f.submitter, &0, &1_000);
    assert_eq!(page.task_ids.len(), MAX_TASKS_PAGE_SIZE);
    assert_eq!(page.total, count);
    assert_eq!(page.next_cursor, Some(MAX_TASKS_PAGE_SIZE));
}

#[test]
fn get_tasks_by_creator_clamps_a_zero_limit_to_an_empty_page() {
    let f = fixture();
    store(&f, &task_id(&f, 1), &f.submitter);

    let page = f.client.get_tasks_by_creator(&f.submitter, &0, &0);
    assert_eq!(page.task_ids.len(), 0);
    assert_eq!(page.total, 1);
    // The caller can still page from the start.
    assert_eq!(page.next_cursor, Some(0));
}

#[test]
fn get_tasks_by_creator_is_empty_for_an_unknown_creator() {
    let f = fixture();
    store(&f, &task_id(&f, 1), &f.submitter);

    let stranger = Address::generate(&f.env);
    let page = f
        .client
        .get_tasks_by_creator(&stranger, &0, &MAX_TASKS_PAGE_SIZE);

    assert_eq!(page.total, 0);
    assert_eq!(page.task_ids.len(), 0);
    assert_eq!(page.next_cursor, None);
}

#[test]
fn get_tasks_by_creator_does_not_leak_other_creators_tasks() {
    let f = fixture();
    let other = Address::generate(&f.env);

    store(&f, &task_id(&f, 1), &f.submitter);
    store(&f, &task_id(&f, 2), &other);

    let mine = f
        .client
        .get_tasks_by_creator(&f.submitter, &0, &MAX_TASKS_PAGE_SIZE);
    let theirs = f
        .client
        .get_tasks_by_creator(&other, &0, &MAX_TASKS_PAGE_SIZE);

    assert_eq!(mine.total, 1);
    assert!(contains(&mine.task_ids, &task_id(&f, 1)));
    assert_eq!(theirs.total, 1);
    assert!(contains(&theirs.task_ids, &task_id(&f, 2)));
}

#[test]
fn a_cursor_past_the_end_returns_an_empty_final_page() {
    let f = fixture();
    store(&f, &task_id(&f, 1), &f.submitter);

    let page = f.client.get_tasks_by_creator(&f.submitter, &99, &10);
    assert_eq!(page.task_ids.len(), 0);
    assert_eq!(page.total, 1);
    assert_eq!(page.next_cursor, None);
}

#[test]
fn the_creator_index_is_bounded_and_evicts_the_oldest_task() {
    let f = fixture();
    let count = MAX_TRACKED_TASKS_PER_CREATOR + 1;
    for i in 0..count {
        store(&f, &task_id(&f, (i + 1) as u8), &f.submitter);
    }

    let page = f
        .client
        .get_tasks_by_creator(&f.submitter, &0, &MAX_TASKS_PAGE_SIZE);

    // The index never grows past its cap, even though `count` tasks were stored.
    assert_eq!(page.total, MAX_TRACKED_TASKS_PER_CREATOR);
    // The first task was evicted; the next-oldest is still indexed.
    assert!(!contains(&page.task_ids, &task_id(&f, 1)));
}

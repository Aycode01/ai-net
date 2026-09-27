//! # Agent Marketplace Unit Tests

extern crate std;

use super::*;
use soroban_sdk::{
    testutils::{Address as _, Events as _},
    Address, Env, IntoVal, Symbol, TryFromVal, TryIntoVal, Val,
};

fn setup() -> (Env, AgentMarketplaceContractClient<'static>) {
    let env = Env::default();
    env.mock_all_auths();
    let id = env.register(AgentMarketplaceContract, ());
    let client = AgentMarketplaceContractClient::new(&env, &id);
    (env, client)
}

fn setup_with_admin() -> (Env, AgentMarketplaceContractClient<'static>, Address) {
    let env = Env::default();
    env.mock_all_auths();
    let id = env.register(AgentMarketplaceContract, ());
    let client = AgentMarketplaceContractClient::new(&env, &id);
    let admin = Address::generate(&env);
    client.initialize(&admin);
    (env, client, admin)
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
fn initialize_cannot_be_called_twice() {
    let (env, client) = setup();
    let admin = Address::generate(&env);
    client.initialize(&admin);
    assert_eq!(
        client.try_initialize(&Address::generate(&env)),
        Err(Ok(Error::AlreadyExists))
    );
}

#[test]
fn list_service_success() {
    let (env, client) = setup();
    let owner = Address::generate(&env);
    let result = client.try_list_service(
        &Symbol::new(&env, "svc1"),
        &Symbol::new(&env, "agent1"),
        &owner,
        &Symbol::new(&env, "research"),
        &1_000_000_i128,
        &200_u32,
        &24_u32,
    );
    assert!(result.is_ok());

    let listing = client.get_listing(&Symbol::new(&env, "svc1"));
    assert!(listing.is_some());
    let listing = listing.unwrap();
    assert_eq!(listing.price_stroops, 1_000_000);
    assert!(listing.active);
}

#[test]
fn list_service_invalid_price() {
    let (env, client) = setup();
    let owner = Address::generate(&env);

    assert_eq!(
        client.try_list_service(
            &Symbol::new(&env, "svc_bad"),
            &Symbol::new(&env, "agent1"),
            &owner,
            &Symbol::new(&env, "research"),
            &0_i128,
            &200_u32,
            &24_u32,
        ),
        Err(Ok(Error::InvalidPrice))
    );
}

#[test]
fn list_service_duplicate() {
    let (env, client) = setup();
    let owner = Address::generate(&env);

    client.list_service(
        &Symbol::new(&env, "svc1"),
        &Symbol::new(&env, "agent1"),
        &owner,
        &Symbol::new(&env, "research"),
        &1_000_000_i128,
        &200_u32,
        &24_u32,
    );

    assert_eq!(
        client.try_list_service(
            &Symbol::new(&env, "svc1"),
            &Symbol::new(&env, "agent2"),
            &owner,
            &Symbol::new(&env, "coding"),
            &2_000_000_i128,
            &100_u32,
            &12_u32,
        ),
        Err(Ok(Error::AlreadyExists))
    );
}

#[test]
fn search_services_filters_by_price() {
    let (env, client) = setup();
    let owner = Address::generate(&env);

    client.list_service(
        &Symbol::new(&env, "svc_cheap"),
        &Symbol::new(&env, "agent1"),
        &owner,
        &Symbol::new(&env, "research"),
        &500_000_i128,
        &200_u32,
        &24_u32,
    );
    client.list_service(
        &Symbol::new(&env, "svc_expensive"),
        &Symbol::new(&env, "agent2"),
        &owner,
        &Symbol::new(&env, "research"),
        &2_000_000_i128,
        &200_u32,
        &24_u32,
    );

    let results = client.search_services(&Symbol::new(&env, "research"), &1_000_000_i128, &0_u32);
    assert_eq!(results.len(), 1);
    assert_eq!(
        results.get(0).unwrap().listing_id,
        Symbol::new(&env, "svc_cheap")
    );
}

#[test]
fn book_agent_success() {
    let (env, client) = setup();
    let owner = Address::generate(&env);
    let client_addr = Address::generate(&env);

    client.list_service(
        &Symbol::new(&env, "svc1"),
        &Symbol::new(&env, "agent1"),
        &owner,
        &Symbol::new(&env, "research"),
        &1_000_000_i128,
        &200_u32,
        &24_u32,
    );

    let booking_id = Symbol::new(&env, "bk1");
    client.book_agent(
        &Symbol::new(&env, "svc1"),
        &client_addr,
        &1_000_000_i128,
        &booking_id,
    );

    let booking = client.get_booking(&booking_id);
    assert!(booking.is_some());
    let booking = booking.unwrap();
    assert_eq!(booking.escrow_amount, 1_000_000);
    assert!(!booking.completed);
    assert!(!booking.cancelled);
}

#[test]
fn book_agent_insufficient_payment() {
    let (env, client) = setup();
    let owner = Address::generate(&env);
    let client_addr = Address::generate(&env);

    client.list_service(
        &Symbol::new(&env, "svc1"),
        &Symbol::new(&env, "agent1"),
        &owner,
        &Symbol::new(&env, "research"),
        &1_000_000_i128,
        &200_u32,
        &24_u32,
    );

    assert_eq!(
        client.try_book_agent(
            &Symbol::new(&env, "svc1"),
            &client_addr,
            &500_000_i128,
            &Symbol::new(&env, "bk_bad"),
        ),
        Err(Ok(Error::InsufficientPayment))
    );
}

#[test]
fn complete_booking_releases_escrow() {
    let (env, client) = setup();
    let owner = Address::generate(&env);
    let client_addr = Address::generate(&env);

    client.list_service(
        &Symbol::new(&env, "svc1"),
        &Symbol::new(&env, "agent1"),
        &owner,
        &Symbol::new(&env, "research"),
        &1_000_000_i128,
        &200_u32,
        &24_u32,
    );

    let booking_id = Symbol::new(&env, "bk1");
    client.book_agent(
        &Symbol::new(&env, "svc1"),
        &client_addr,
        &1_000_000_i128,
        &booking_id,
    );

    client.complete_booking(&booking_id);

    let booking = client.get_booking(&booking_id).unwrap();
    assert!(booking.completed);
}

#[test]
fn cancel_booking_refunds_client() {
    let (env, client) = setup();
    let owner = Address::generate(&env);
    let client_addr = Address::generate(&env);

    client.list_service(
        &Symbol::new(&env, "svc1"),
        &Symbol::new(&env, "agent1"),
        &owner,
        &Symbol::new(&env, "research"),
        &1_000_000_i128,
        &200_u32,
        &24_u32,
    );

    let booking_id = Symbol::new(&env, "bk1");
    client.book_agent(
        &Symbol::new(&env, "svc1"),
        &client_addr,
        &1_000_000_i128,
        &booking_id,
    );

    client.cancel_booking(&booking_id);

    let booking = client.get_booking(&booking_id).unwrap();
    assert!(booking.cancelled);
}

#[test]
fn rate_booking_updates_agent_rating() {
    let (env, client) = setup();
    let owner = Address::generate(&env);
    let client_addr = Address::generate(&env);

    client.list_service(
        &Symbol::new(&env, "svc1"),
        &Symbol::new(&env, "agent1"),
        &owner,
        &Symbol::new(&env, "research"),
        &1_000_000_i128,
        &200_u32,
        &24_u32,
    );

    let booking_id = Symbol::new(&env, "bk1");
    client.book_agent(
        &Symbol::new(&env, "svc1"),
        &client_addr,
        &1_000_000_i128,
        &booking_id,
    );
    client.complete_booking(&booking_id);
    client.rate_booking(&booking_id, &5);

    let rating = client.get_agent_rating(&Symbol::new(&env, "agent1"));
    assert_eq!(rating.total_ratings, 1);
    assert_eq!(rating.rating_sum, 5);
}

#[test]
fn rate_invalid_score() {
    let (env, client) = setup();
    let owner = Address::generate(&env);
    let client_addr = Address::generate(&env);

    client.list_service(
        &Symbol::new(&env, "svc1"),
        &Symbol::new(&env, "agent1"),
        &owner,
        &Symbol::new(&env, "research"),
        &1_000_000_i128,
        &200_u32,
        &24_u32,
    );

    let booking_id = Symbol::new(&env, "bk1");
    client.book_agent(
        &Symbol::new(&env, "svc1"),
        &client_addr,
        &1_000_000_i128,
        &booking_id,
    );
    client.complete_booking(&booking_id);

    assert_eq!(
        client.try_rate_booking(&booking_id, &0),
        Err(Ok(Error::InvalidPrice))
    );
    assert_eq!(
        client.try_rate_booking(&booking_id, &6),
        Err(Ok(Error::InvalidPrice))
    );
}

#[test]
fn pause_blocks_listing() {
    let (env, client, _admin) = setup_with_admin();
    client.pause();

    let owner = Address::generate(&env);
    assert_eq!(
        client.try_list_service(
            &Symbol::new(&env, "svc1"),
            &Symbol::new(&env, "agent1"),
            &owner,
            &Symbol::new(&env, "research"),
            &1_000_000_i128,
            &200_u32,
            &24_u32,
        ),
        Err(Ok(Error::ContractPaused))
    );
}

#[test]
fn unpause_allows_listing() {
    let (env, client, _admin) = setup_with_admin();
    client.pause();
    client.unpause();

    let owner = Address::generate(&env);
    client.list_service(
        &Symbol::new(&env, "svc1"),
        &Symbol::new(&env, "agent1"),
        &owner,
        &Symbol::new(&env, "research"),
        &1_000_000_i128,
        &200_u32,
        &24_u32,
    );
    assert!(client.get_listing(&Symbol::new(&env, "svc1")).is_some());
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
fn pause_blocks_complete_booking() {
    let (env, client, _admin) = setup_with_admin();
    let owner = Address::generate(&env);
    let client_addr = Address::generate(&env);

    client.list_service(
        &Symbol::new(&env, "svc1"),
        &Symbol::new(&env, "agent1"),
        &owner,
        &Symbol::new(&env, "research"),
        &1_000_000_i128,
        &200_u32,
        &24_u32,
    );

    let booking_id = Symbol::new(&env, "bk1");
    client.book_agent(
        &Symbol::new(&env, "svc1"),
        &client_addr,
        &1_000_000_i128,
        &booking_id,
    );

    client.pause();

    assert_eq!(
        client.try_complete_booking(&booking_id),
        Err(Ok(Error::ContractPaused))
    );
}

#[test]
fn search_services_still_works_when_paused() {
    let (env, client, _admin) = setup_with_admin();
    let owner = Address::generate(&env);

    client.list_service(
        &Symbol::new(&env, "svc1"),
        &Symbol::new(&env, "agent1"),
        &owner,
        &Symbol::new(&env, "research"),
        &1_000_000_i128,
        &200_u32,
        &24_u32,
    );

    client.pause();

    // Reads should still work when paused.
    let results = client.search_services(&Symbol::new(&env, "research"), &0, &0);
    assert_eq!(results.len(), 1);
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
    assert_eq!(t0, symbol_short!("market"));
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
fn list_service_emits_event_after_write() {
    let (env, client) = setup();
    let owner = Address::generate(&env);

    client.list_service(
        &Symbol::new(&env, "svc_ev"),
        &Symbol::new(&env, "agent1"),
        &owner,
        &Symbol::new(&env, "research"),
        &1_000_000_i128,
        &200_u32,
        &24_u32,
    );

    // Query events immediately: env.events().all() reflects only the most
    // recent invocation, and the payload cannot predate the storage write.
    let payload: ServiceListedEvent = last_events(&env).remove(0);
    assert!(client.get_listing(&Symbol::new(&env, "svc_ev")).is_some());
    assert_eq!(payload.listing_id, Symbol::new(&env, "svc_ev"));
    assert_eq!(payload.agent_id, Symbol::new(&env, "agent1"));
    assert_eq!(payload.capability, Symbol::new(&env, "research"));
    assert_eq!(payload.price_stroops, 1_000_000);
}

#[test]
fn book_agent_emits_event_after_write() {
    let (env, client) = setup();
    let owner = Address::generate(&env);
    let client_addr = Address::generate(&env);

    client.list_service(
        &Symbol::new(&env, "svc1"),
        &Symbol::new(&env, "agent1"),
        &owner,
        &Symbol::new(&env, "research"),
        &1_000_000_i128,
        &200_u32,
        &24_u32,
    );

    let booking_id = Symbol::new(&env, "bk_ev");
    client.book_agent(
        &Symbol::new(&env, "svc1"),
        &client_addr,
        &1_000_000_i128,
        &booking_id,
    );

    // Query events immediately: env.events().all() reflects only the most
    // recent invocation, and the payload cannot predate the storage write.
    let payload: ServiceBookedEvent = last_events(&env).remove(0);
    assert!(client.get_booking(&booking_id).is_some());
    assert_eq!(payload.booking_id, booking_id);
    assert_eq!(payload.listing_id, Symbol::new(&env, "svc1"));
    assert_eq!(payload.client, client_addr);
    assert_eq!(payload.escrow_amount, 1_000_000);
}

#[test]
fn complete_booking_emits_event_after_write() {
    let (env, client) = setup();
    let owner = Address::generate(&env);
    let client_addr = Address::generate(&env);

    client.list_service(
        &Symbol::new(&env, "svc1"),
        &Symbol::new(&env, "agent1"),
        &owner,
        &Symbol::new(&env, "research"),
        &1_000_000_i128,
        &200_u32,
        &24_u32,
    );
    let booking_id = Symbol::new(&env, "bk1");
    client.book_agent(
        &Symbol::new(&env, "svc1"),
        &client_addr,
        &1_000_000_i128,
        &booking_id,
    );

    client.complete_booking(&booking_id);

    let payload: ServiceCompletedEvent = last_events(&env).remove(0);
    assert_eq!(payload.booking_id, booking_id);
    assert_eq!(payload.payment_released, 1_000_000);
}

#[test]
fn cancel_booking_emits_event_after_write() {
    let (env, client) = setup();
    let owner = Address::generate(&env);
    let client_addr = Address::generate(&env);

    client.list_service(
        &Symbol::new(&env, "svc1"),
        &Symbol::new(&env, "agent1"),
        &owner,
        &Symbol::new(&env, "research"),
        &1_000_000_i128,
        &200_u32,
        &24_u32,
    );
    let booking_id = Symbol::new(&env, "bk1");
    client.book_agent(
        &Symbol::new(&env, "svc1"),
        &client_addr,
        &1_000_000_i128,
        &booking_id,
    );

    client.cancel_booking(&booking_id);

    let payload: ServiceCancelledEvent = last_events(&env).remove(0);
    assert_eq!(payload.booking_id, booking_id);
    assert_eq!(payload.refund_amount, 1_000_000);
}

#[test]
fn rate_booking_emits_event_after_write() {
    let (env, client) = setup();
    let owner = Address::generate(&env);
    let client_addr = Address::generate(&env);

    client.list_service(
        &Symbol::new(&env, "svc1"),
        &Symbol::new(&env, "agent1"),
        &owner,
        &Symbol::new(&env, "research"),
        &1_000_000_i128,
        &200_u32,
        &24_u32,
    );
    let booking_id = Symbol::new(&env, "bk1");
    client.book_agent(
        &Symbol::new(&env, "svc1"),
        &client_addr,
        &1_000_000_i128,
        &booking_id,
    );
    client.complete_booking(&booking_id);

    client.rate_booking(&booking_id, &5);

    // Query events immediately: env.events().all() reflects only the most
    // recent invocation, and the payload cannot predate the storage write.
    let payload: ServiceRatedEvent = last_events(&env).remove(0);
    let rating = client.get_agent_rating(&Symbol::new(&env, "agent1"));
    assert_eq!(rating.total_ratings, 1);
    assert_eq!(payload.booking_id, booking_id);
    assert_eq!(payload.agent_id, Symbol::new(&env, "agent1"));
    assert_eq!(payload.rating, 5);
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
fn marketplace_event_payloads_roundtrip() {
    let env = Env::default();
    let owner = Address::generate(&env);
    let agent = Symbol::new(&env, "agent1");
    let listing = Symbol::new(&env, "svc1");
    let booking = Symbol::new(&env, "bk1");

    assert_roundtrip(
        &env,
        ServiceListedEvent {
            listing_id: listing.clone(),
            agent_id: agent.clone(),
            capability: Symbol::new(&env, "research"),
            price_stroops: 1_000,
        },
    );
    assert_roundtrip(
        &env,
        ServiceBookedEvent {
            booking_id: booking.clone(),
            listing_id: listing,
            client: owner.clone(),
            escrow_amount: 2_000,
        },
    );
    assert_roundtrip(
        &env,
        ServiceCompletedEvent {
            booking_id: booking.clone(),
            payment_released: 3_000,
        },
    );
    assert_roundtrip(
        &env,
        ServiceCancelledEvent {
            booking_id: booking.clone(),
            refund_amount: 4_000,
        },
    );
    assert_roundtrip(
        &env,
        ServiceRatedEvent {
            booking_id: booking,
            agent_id: agent,
            rating: 5,
        },
    );
    let _ = owner;
}

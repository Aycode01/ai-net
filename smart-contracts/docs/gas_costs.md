# Soroban Gas Estimates

Estimates are CPU instruction counts (CU) returned by each contract's
`estimate_gas` entry point and the shared `estimate` interface. They are
calibrated against `Env::cost_estimate().budget().cpu_instruction_cost()` in
native contract integration tests. Soroban native tests do not include all Wasm
execution costs; use transaction simulation before submission when an exact
network resource quote is required.

`count` is zero-safe. Batch formulas charge transaction overhead once and then
add marginal item work. The `estimate` map interface uses one map entry per
item; a scalar estimate call is available where a contract exposes
`estimate_gas(operation, count)`.

## Agent Registry

| Operation | Single estimate | Batched estimate | Cost drivers |
|---|---:|---:|---|
| `register_agent` | 170,000 CU | `170,000 + 130,000 × (n − 1)` CU | Pause/frozen checks, instance counters, capability index and agent persistent writes, index write, TTL, two events. |
| `resolve_error` | 42,000 CU | `42,000 + 22,000 × (n − 1)` CU | Error record read and write, validation, TTL, resolution event. |
| `cleanup_expired_errors` | 16,000 CU | `16,000 + 8,000 × (n − 1)` CU | Error reads and removals for the supplied IDs. |
| `slash_bond` | 52,000 CU | Flat per call | Agent read/write, TTL, bond event. |
| `deregister_with_bond` | 68,000 CU | Flat per call | Agent and capability-index update, cooldown/bond state, TTL and events. |

Registration was measured at 169,441 CU for one item and 356,916 CU for two
items; the estimates above fall within the integration-test tolerance for both.

## Agent Bidding

| Operation | Single estimate | Batched estimate | Cost drivers |
|---|---:|---:|---|
| `bid` (`submit_bid`) | 151,000 CU | `151,000 × n` CU for separate submissions | Auction read/write, duplicate check, sealed bid write, bidder-list update, TTL and event. There is no batch submit method. |
| `reveal` (`reveal_bid`) | 181,000 CU | `181,000 × n` CU for separate reveals | Auction and bid reads/writes, commitment XDR/hash verification, TTL and event. |
| `finalize` (`reveal_bids`) | 150,000 CU at n=1 | `150,000 + 133,000 × (n − 1)` CU | Bidder-list scan, revealed bid reads, score calculation, winner write and event. |
| `dispute` | Not supported | Not supported | This contract has no dispute operation; disputes are handled by `dispute_resolution`. |

Measured examples: one bid submission 151,375 CU; one reveal 182,667 CU; and
finalizing two revealed bids 282,930 CU.

## Dispute Resolution

| Operation | Single estimate | Batched estimate | Cost drivers |
|---|---:|---:|---|
| `dispute` (`file_dispute`) | 85,000 CU for five jurors | `45,000 + 8,000 Ã— juror_count` CU, capped at five jurors | Active juror instance read, bounded juror selection, dispute record write, and event. |

Measured native test cost with five jurors: 86,316 CU.

## Agent Marketplace

| Operation | Single estimate | Batched estimate | Cost drivers |
|---|---:|---:|---|
| `listing` (`list_service`) | 63,000 CU | `63,000 × n` CU for separate listings | Duplicate check, listing write, capability-index read/write and event. There is no batch listing method. |
| `search` (`search_services`) | `20,000 + 27,000 × n` CU | Same formula; n is listings scanned | Capability index read and one listing read/filter per ID. |
| `purchase` (`book_agent`) | 73,000 CU | `73,000 × n` CU for separate bookings | Listing read, booking existence check/write and event. |

Measured examples: listing 75,390 CU, booking 73,021 CU, searching one listing
47,567 CU, and searching two listings 75,577 CU.

## Task Store

| Operation | Single estimate | Batched estimate | Cost drivers |
|---|---:|---:|---|
| `create` (`store_task_metadata`) | 85,000 CU | `85,000 × n` CU for separate creates | Duplicate check, task write, TTL and event; optional oracle configuration adds cross-contract work. |
| `update` (`update_task_status`) | 104,000 CU | `104,000 × n` CU for separate updates | Task read/write, assigned-agent membership and transition checks, lifecycle event. |
| `query` (`get_task_metadata`) | `38,000 + 24,000 × n` CU | Same formula; n is records read by `get_task_metadata_batch` | Task reads and expiry checks. |

Measured examples: create 88,859 CU, update 104,078 CU, one task query 53,604 CU,
and a two-task batch query 90,137 CU.

## Token Transfers (escrow)

`agent_marketplace` (`book_agent`, `complete_booking`, `cancel_booking`) and
`agent_bidding` (`create_auction`, `submit_bid`, `award_contract`,
`abort_auction`, `claim_refund`, `claim_bid_refund`, `release_escrow`,
`refund_escrow`) now perform cross-contract Stellar Asset Contract calls: one
`decimals()` check plus one `transfer` per moved amount. `award_contract` and
`abort_auction` transfer once per bidder (bounded by `MAX_BIDDERS`) plus once
to the creator.

The per-operation figures in the tables above predate these transfers and
**understate** the affected operations. They must be re-measured with
`cargo test -p agent-marketplace -p agent-bidding` against the
`Env::cost_estimate()` budget and the `gas.rs` constants updated; until then,
always use transaction simulation for these calls.

## Runtime Budget Guard

Soroban SDK 22 exposes budget consumption through test utilities, not to
contract execution. Contracts cannot inspect a caller's remaining CPU budget
with `Env`; the network validates the transaction's declared resource limits.
The estimate entry points let callers estimate before submission, while a
contract-side `require_gas_budget(minimum)` cannot truthfully check remaining
budget in this SDK version.

# Paid Agent Runtime Trace Schema

This schema makes `docs/spec/PaidAgentRuntime.tla` load-bearing against the
runtime implementation. Each JSONL row records one critical decision for one
opaque payer-agent-community scope.

Rows contain only opaque identifiers, boolean authorization results, integer
millisecond durations, and aggregate ledger counts. Rows must not contain
keys, pubkeys, raw Nostr events, channel IDs, offers, wallet notes, invoices,
or provider data.

Schema version: 1

## Modeled seam

The seam is the paid-runtime lifecycle decision boundary in `buzz-acp`: ledger
replay, reservation creation and binding, invocation dispatch, prompt metering,
checkpointing, and settlement. The production emitter is
`crates/buzz-acp/src/runtime_conformance.rs`. Wallet verification emits the
payment and deposit actions through the replayed ledger boundary.

## Abstract state

| Field | TLA+ state | Meaning |
|---|---|---|
| `credited_ms` | deposited credit | Verified runtime milliseconds |
| `used_ms` | settled usage | Final billed milliseconds |
| `locked_ms` | open reservation caps | Credit unavailable to another reservation |
| `open_reservations` | reservation status | Unsettled reservation count |
| `active_meters` | meter status | Prompts currently inside the billable boundary |

All identifiers are one-way opaque hashes. The projection contains no keys,
pubkeys, channel IDs, raw events, offers, notes, invoices, or wallet data.

## Actions

| Trace action | TLA+ action | Critical |
|---|---|---|
| `quote_requested` | `RequestQuote` | yes |
| `payment_settled` | `SettlePayment` | yes |
| `credit_deposited` | `DepositCredit` | yes |
| `runtime_reserved` | `ReserveRuntime` | yes |
| `instruction_bound` | `BindInstruction` | yes |
| `invocation_dispatched` | `AdmitInvocation` | yes |
| `meter_started`, `meter_paused`, `meter_resumed` | meter transitions | yes |
| `meter_checkpointed` | `CheckpointMeter` | yes |
| `reservation_settled` | `SettleReservation` | yes |
| `duplicate_reused` | `ReuseDuplicate` | yes |
| `invocation_rejected` | `RejectInvocation` | yes |
| `impl_bug` | coverage failure witness | yes |

## Failure policy

The independent checker is
`buzz_conformance::paid_agent_runtime::check_runtime_trace`. It rejects credit
without verified settlement, conflicting duplicates, reservation overspend,
invalid external invocation, metering outside `session/prompt`, usage above a
cap, dispatch without the exact bound instruction, repeated dispatch, crash
billing past a checkpoint, state-before or state-after mismatches, unknown
schema versions, explicit `impl_bug`, and missing required critical actions.

The `buzz-acp` test
`production_emitter_trace_is_accepted_by_independent_checker` generates JSONL
through the production projection and immediately checks it with the
independent reducer. Hand-written negative fixtures and property tests prove
the checker rejects forbidden mutations.

Run the checker directly with:

```bash
cargo run -p buzz-conformance --bin check-paid-runtime-trace < trace.jsonl
```

Trace conformance checks exercised executions. It does not prove unexecuted
paths, liveness, cryptography, relay delivery, monotonic-clock correctness, or
provider settlement correctness.

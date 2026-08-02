# Runtime Wallet Trace Schema

Schema version: **1** (`WALLET_TRACE_SCHEMA_VERSION` in
`crates/buzz-conformance/src/wallet.rs`).

## Modeled seam

The seam is the identity-scoped, durable payment-attempt checkpoint used by
`wallet_send` and `wallet_send_profile_zap`. The formal contract is
`docs/spec/WalletPaymentAttempts.tla`: the implementation must persist
`Paying` before its only provider send call, and every later invocation must
reconcile instead of sending again.

## Abstract state

| Field | Spec variable | Meaning | Secret-free rationale |
|---|---|---|---|
| `attempt_id` | product-machine key | Stable SHA-256 prefix for one request UUID | Does not contain the request, destination, payer, or provider ID |
| `status` | `status` | Exact durable generic/profile checkpoint state | Closed enum; contains no payment material |
| `payment_recorded` | `paymentRecorded` | Whether a provider result is stored | Boolean only; no amount, status message, or provider ID |

The projection deliberately does not contain private keys, seeds, recovery
phrases, invoices, offers, payer notes, comments, pubkeys, event bodies,
signatures, payment hashes, provider identifiers, amounts, or timestamps.
It maps the persisted enum without clamping or repairing it.

## Actions

| Runtime action | Spec action | Critical? |
|---|---|---|
| `prepare_generic` | `PrepareGeneric` | yes |
| `prepare_profile` | `PrepareProfile` | yes |
| `begin_dispatch` | `BeginDispatch` | yes |
| `reconcile` | `Reconcile` | yes |
| `record_pending` | `RecordPending` | yes |
| `record_completed` | `RecordCompleted` | yes |
| `record_paid_without_proof` | `RecordPaidWithoutProof` | yes |
| `record_failed` | `RecordFailed` | yes |
| `reuse_terminal` | `ReuseTerminal` | yes |
| `reject_conflict` | `RejectConflict` | yes |

`begin_dispatch` is recorded after the `Paying` checkpoint is committed and
before provider I/O. A `Paying` retry emits `reconcile`; another
`begin_dispatch` is illegal.

## Failure policy

The independent checker fails on an illegal transition, a mismatch in either
projected state, an unknown or malformed critical action, an explicit
`impl_bug`, an empty trace, or a scenario-required action that is absent.
The checker advances from its computed state, never from implementation output.

## Examples

Valid:

```json
{"schema_version":1,"attempt_id":"attempt-001","action":{"type":"prepare_generic"},"state_before":{"status":"absent","payment_recorded":false},"state_after":{"status":"generic_prepared","payment_recorded":false}}
{"schema_version":1,"attempt_id":"attempt-001","action":{"type":"begin_dispatch"},"state_before":{"status":"generic_prepared","payment_recorded":false},"state_after":{"status":"generic_paying","payment_recorded":false}}
```

Invalid (a second provider dispatch):

```json
{"schema_version":1,"attempt_id":"attempt-001","action":{"type":"prepare_generic"},"state_before":{"status":"absent","payment_recorded":false},"state_after":{"status":"generic_prepared","payment_recorded":false}}
{"schema_version":1,"attempt_id":"attempt-001","action":{"type":"begin_dispatch"},"state_before":{"status":"generic_prepared","payment_recorded":false},"state_after":{"status":"generic_paying","payment_recorded":false}}
{"schema_version":1,"attempt_id":"attempt-001","action":{"type":"begin_dispatch"},"state_before":{"status":"generic_paying","payment_recorded":false},"state_after":{"status":"generic_paying","payment_recorded":false}}
```

Replay a trace:

```bash
cargo run -p buzz-conformance --bin check-wallet-trace -- \
  path/to/trace.jsonl begin_dispatch reconcile
```

Set `BUZZ_WALLET_TRACE_PATH` before running desktop integration flows to write
the same secret-free JSONL emitted at the live persistence boundary.

## Limits

Trace checking proves only that **executed paths** emitted traces accepted by
this model. It does not prove unexecuted paths, provider behavior, filesystem
durability, lock correctness, Tauri IPC, relay publication, BOLT11/BOLT12
parsing, payment correctness, or cryptography. It trusts the implementation
projection and emit placement. The `reject_conflict` action records the
decision but the abstract projection does not prove request-detail comparison.
Integration and property tests widen execution coverage; a future proof can
target the pure transition/projection core after the seam stabilizes.

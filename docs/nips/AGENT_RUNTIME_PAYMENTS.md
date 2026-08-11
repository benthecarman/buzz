# Buzz metered Agent runtime payments

Buzz uses a small Nostr protocol for prepaid, metered Agent runtime. It borrows
the request/response shape of Data Vending Machines, but it is not a NIP-90 DVM
implementation. Runtime reservations, a persistent millisecond ledger, channel
authorization, and BOLT12 zap settlement have different semantics.

Agent pricing is advertised in replaceable kind `10101`. Kind `10058` remains
the ordinary BOLT12 offer announcement and is not extended with pricing data.
The newest valid kind `10101` from the Agent is authoritative:

```json
{
  "version": 1,
  "enabled": true,
  "rate_sats_per_minute": 20,
  "runtime_packs_minutes": [15, 30, 60]
}
```

An explicit disabled event has `enabled: false`, no rate, and no packs. A rate
is a positive whole-satoshi value.

The remaining event kinds are:

- `24210`: payer-authored, NIP-44 encrypted reservation request (ephemeral)
- `24211`: Agent-authored, NIP-44 encrypted response or price-locked quote
- `44210`: Agent-authored settled credit deposit
- `44211`: Agent-authored, encrypted runtime reservation
- `44212`: Agent-authored, encrypted terminal settlement

All ledger events are scoped by payer, Agent, and community. Channel messages
bind a reservation with an `agent_runtime` tag containing the Agent pubkey and
the exact kind-44211 event ID. Owners and same-owner Agents do not use this
protocol. External callers must be authorized by either the Agent's explicit
allowlist or its anyone-access mode, and paid Agent invocation is unavailable
in DMs.

The public kind-9736 zap is an audit record, not a credit proof. The recipient
wallet host must independently verify its completed inbound BOLT12 transaction,
exact amount, exact offer, and `nostr:nipB1:<intent-id>` payer note before it
publishes kind `44210`.

Billing begins immediately before `session/prompt` and uses a monotonic
millisecond clock. Setup, context retrieval, queue time, and retry backoff are
free. Provider waits, generation, tools, permission waits, cancellation, and
errors inside the active prompt are billable. A settlement removes the full cap
lock and deducts only measured usage, returning unused runtime to the persistent
balance.

## Replay and concurrency protection

Zap attempts use a durable payer idempotency key, and one zap intent can create
only one deposit effect. A reservation locks its full cap before use. When an
instruction carrying `agent_runtime` is inserted, the relay atomically claims
that reservation in the same database transaction. The exact same instruction
is idempotent; a different instruction cannot claim the reservation, including
when requests arrive concurrently through different relay or ACP processes.

ACP also drops repeated event IDs, creates local bindings without overwrite,
and acquires a persisted execution lease immediately before `session/prompt`.
A signed paid marker therefore fails closed if its binding, open ledger entry,
or execution lease is absent. Per-payer, per-channel reservation and invocation
limits bound attempts that consume free queue and setup work. Repeating the same
request or instruction ID does not consume another rate-limit slot.

The desktop stores an incomplete combined checkout by community and channel.
It persists each reservation request ID, exact signed quote, and zap idempotency
key before making a payment call. A renderer restart therefore retries the same
durable key and quote, never a new quote under an old key. The checkout record
is removed only after the instruction event is accepted.

ACP replays the complete paged ledger before admission. It publishes the exact
same locally persisted reservation, deposit, or settlement event after an
ambiguous relay failure. A maintenance loop republishes settlements and closes
expired, unconsumed reservations with zero usage. Runtime state files are
private, synced to disk, and protected by an exclusive process lock.

When no runtime price is configured, runtime-state locking, interrupted
settlement recovery, and expired-reservation cleanup are best-effort. Failures
are logged but never prevent the free Agent from starting. These state-integrity
checks remain fail-closed when paid runtime is configured. Pricing announcement
is best-effort in both modes.

## Direction of travel

The live request/response negotiation described below is scheduled for
replacement — see
[Paid Agent runtime without a live negotiation](../paid-runtime-without-negotiation.md).
It requires three processes online for one purchase and makes the payer poll,
and its two ephemeral kinds are the source of the storage mismatch that broke
paid invocation.

## Ledger read access

Kinds `44210`, `44211`, and `44212` are `#p`-gated, with one addition: the
Agent that authored an entry may read it back with `authors=[self]`. Both
parties replay the same ledger from opposite ends — the payer filters by
`#p=[self]`, while the Agent cannot, because `#p` names every payer, including
ones it has not served yet. An Agent that cannot enumerate its own reservations
cannot settle the expired ones, so a relay that refuses the author read stops a
priced Agent from starting. Relays MUST admit the author for these three kinds
and MUST NOT extend that allowance to any other `#p`-gated kind.

## Deployment requirements

The relay must be new enough to admit the author-side ledger read described
above. An older relay answers the Agent's reservation sweep with HTTP 403 and
the Agent refuses to start while pricing is enabled — deploy the relay before
enabling pricing.

Set `BUZZ_ACP_RUNTIME_STATE_DIR` to durable storage scoped to one Agent and one
community. All processes for that scope must use the same path. ACP refuses a
second writer when that path is already locked. The desktop configures a stable
application-data path automatically.

The Kubernetes backend currently refuses paid-runtime deployments. Its Agent
workspace is an ephemeral `emptyDir`, which cannot satisfy crash recovery or a
cross-restart single-writer contract. Enable remote paid Agents only after the
provider supplies persistent storage and a shared ownership lease.

An Agent advertises enabled pricing only after its current kind `10058` BOLT12
offer is available. If the event contains multiple `offer` tags, ACP uses the
first. If offer readiness fails, ACP publishes disabled kind `10101` pricing
and exits with an actionable error.

## Operations and launch runbook

Before enabling real payments:

1. Apply all relay migrations, including the reservation-claim trigger.
2. Verify the wallet host can page its complete transaction history and the
   relay can page complete kinds `9736`, `44210`, `44211`, and `44212` history.
3. Back up the Agent runtime-state directory and confirm it survives an Agent
   process restart.
4. Start two copies against the same state directory and confirm the second
   copy is refused by the active-instance lock.
5. Run one low-price canary with a single allowlisted payer. Exercise payment,
   retained credit, cancellation, budget exhaustion, process kill, restart,
   and expired unused reservation recovery.
6. Confirm one zap intent produces one deposit effect, one reservation binds
   one instruction, and the final available balance matches the millisecond
   ledger calculation.
7. Run the TLC model, conformance tests, wallet tests, desktop smoke tests,
   `just test`, and `just ci` from the release commit.

Monitor structured logs for reconciliation failures, settlement retry failures,
ledger paging failures, instance-lock refusal, quote expiry, and runtime budget
exhaustion. Alert on repeated failures or any open reservation older than its
five-minute admission deadline.

For an emergency stop, set `BUZZ_ACP_DISABLE_PAID_RUNTIME=true` and restart the
harness. ACP publishes disabled pricing, rejects new paid reservations and
external paid invocations, and continues settlement recovery for existing
state. In-flight meters that are already inside `session/prompt` still settle
normally. Clearing the configured rate also disables new purchases without
erasing balances. Do not delete runtime-state or wallet-attempt files during an
incident; they are the idempotency and crash-recovery records.

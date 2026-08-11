# Buzz prepaid Agent runtime payments

Buzz meters paid Agent runtime with a small Nostr protocol built on one idea:
**the payer buys against published terms, and both parties settle against a
durable ledger.** There is no negotiation. An Agent that is offline can still
sell runtime; only doing the work requires it to be awake.

## Published terms

Three durable, agent-signed events describe the terms completely:

| Kind    | Carries                                             |
|---------|-----------------------------------------------------|
| `10101` | the per-runtime-minute rate and packs (replaceable) |
| `10058` | the BOLT12 offer payment settles against            |
| `10100` | access mode, allowlist, and channels                |

Pricing (kind `10101`) is authoritative in its newest valid event:

```json
{
  "version": 1,
  "enabled": true,
  "rate_sats_per_minute": 20,
  "runtime_packs_minutes": [15, 30, 60]
}
```

An explicit disabled event has `enabled: false`, no rate, and no packs. A rate
is a positive whole-satoshi value. Kind `10058` remains the ordinary BOLT12
offer announcement and is never extended with pricing data.

## Ledger kinds

- `44210`: Agent-authored settled credit deposit
- `44211`: Agent-authored, encrypted runtime reservation
- `44212`: Agent-authored, encrypted terminal settlement

All ledger events are scoped by payer, Agent, and community with `p` and `h`
tags. Channel messages bind a reservation with an `agent_runtime` tag
containing the Agent pubkey and the exact kind-44211 event ID. Owners and
same-owner Agents do not use this protocol. External callers must be
authorized by the Agent's allowlist or anyone-access mode, and paid invocation
is unavailable in DMs.

## Purchase

1. The payer reads kinds `10101`, `10058`, and `10100`, confirms it is in the
   Agent's audience, and computes `rate × pack`.
2. The payer zaps the offer. The zap intent carries an
   `agent_runtime_purchase` tag holding a `RuntimePurchase`: the channel, the
   pack, the rate, the amount, and **the exact signed pricing event paid
   against**, embedded so attestation never depends on a relay fetch.
3. The wallet host (the owner's desktop, which holds the wallet the payment
   lands in) independently verifies the completed inbound BOLT12 transaction,
   exact amount, exact offer, and `nostr:nipB1:<intent-id>` payer note, and
   validates the purchase against the pinned agent-signed pricing. It then
   publishes the kind `44210` deposit, tagged with the pricing event id.

A pinned rate is honored even if the Agent has since repriced: the Agent's own
signature advertised it, and there is no refund path — crediting at the signed
rate is the bounded-harm choice. The public kind-9736 zap remains an audit
record, not a credit proof.

## Reservations are minted from credit, not requested

The Agent's maintenance loop (every ~30 seconds) keeps **one open reservation
per funded (payer, channel) scope**:

- It enumerates funded scopes from its own deposits — readable because relays
  admit the ledger author (see *Ledger read access*).
- Before minting it re-checks the payer's access and that the channel is a
  non-DM group containing both parties. A payer who loses access keeps their
  credit but stops receiving new locks.
- The cap is `min(available credit, last purchased pack)`. Caps are bounds,
  not pack SKUs: retained credit is fractional after any settlement, and a
  reservation over exactly the remaining balance must be expressible. Validity
  is thirty days (`RESERVATION_VALIDITY_SECS`), refreshed in its second half.
- A lock is settled unused and replaced on the next pass when new credit or a
  larger pack means a bigger cap should be locked.
- An ambiguous relay submit is persisted first and republished verbatim, so a
  scope can never hold two live locks over the same credit.

The payer discovers its reservation by reading its own ledger (`#p = self`) —
the same query that shows the balance. First purchases resolve within the
attestation loop (~15s) plus the mint loop (~30s); repeat invocations against
retained credit attach the waiting reservation immediately.

## Invocation and metering

When an instruction carrying `agent_runtime` is inserted, the relay atomically
claims that reservation in the same database transaction. The same instruction
is idempotent; a different instruction cannot claim the reservation, including
across concurrent relay or ACP processes.

Billing begins immediately before `session/prompt` and uses a monotonic
millisecond clock. Setup, context retrieval, queue time, and retry backoff are
free. Provider waits, generation, tools, permission waits, cancellation, and
errors inside the active prompt are billable. A settlement removes the full
cap lock and deducts only measured usage, returning unused runtime to the
persistent balance.

ACP drops repeated event IDs, creates local bindings without overwrite, and
acquires a persisted execution lease immediately before `session/prompt`. A
signed paid marker fails closed if its binding, open ledger entry, or
execution lease is absent. The maintenance loop republishes settlements and
closes expired unconsumed reservations with zero usage. Runtime state files
are private, synced to disk, and protected by an exclusive process lock.

When no runtime price is configured, runtime-state locking, interrupted
settlement recovery, and cleanup are best-effort and never prevent the free
Agent from starting. They remain fail-closed when paid runtime is configured.

## Ledger read access

Kinds `44210`, `44211`, and `44212` are `#p`-gated, with one addition: the
Agent that authored an entry may read it back with `authors=[self]`. Both
parties replay the same ledger from opposite ends — the payer filters by
`#p=[self]`, while the Agent cannot, because `#p` names every payer, including
ones it has not served yet. Relays MUST admit the author for these three kinds
and MUST NOT extend that allowance to any other `#p`-gated kind.

## Deployment requirements

The relay must admit the author-side ledger read described above and must not
require any negotiation kinds — the retired ephemeral kinds `24210`/`24211`
are gone from the protocol entirely.

Set `BUZZ_ACP_RUNTIME_STATE_DIR` to durable storage scoped to one Agent and
one community. All processes for that scope must use the same path. ACP
refuses a second writer when that path is already locked. The desktop
configures a stable application-data path automatically.

The Kubernetes backend currently refuses paid-runtime deployments. Its Agent
workspace is an ephemeral `emptyDir`, which cannot satisfy crash recovery or a
cross-restart single-writer contract.

An Agent advertises enabled pricing only after its current kind `10058` BOLT12
offer is available. If the event contains multiple `offer` tags, ACP uses the
first. If offer readiness fails, ACP publishes disabled kind `10101` pricing
and exits with an actionable error.

## Known limits

- **Attestation availability.** Only the wallet host can see the payment
  land, so the owner's desktop remains in the purchase path even though the
  Agent does not. Removing it means an Agent-held wallet or relay-side BOLT12
  verification — a different trust model, deliberately out of scope here.
- **No refunds.** Credit is spendable, retained, and non-refundable. Access
  revocation stops new locks but does not return sats.

## Operations and launch runbook

Before enabling real payments:

1. Apply all relay migrations, including the reservation-claim trigger.
2. Verify the wallet host can page its complete transaction history and the
   relay can page complete kinds `9736`, `44210`, `44211`, and `44212`
   history.
3. Back up the Agent runtime-state directory and confirm it survives an Agent
   process restart.
4. Start two copies against the same state directory and confirm the second
   copy is refused by the active-instance lock.
5. Run one low-price canary with a single allowlisted payer. Exercise
   payment, retained credit, budget exhaustion, process kill, restart, and
   expired unused reservation recovery.
6. Confirm one zap intent produces one deposit effect, one reservation binds
   one instruction, and the final available balance matches the millisecond
   ledger calculation.
7. Run the TLC model, conformance tests, wallet tests, desktop smoke tests,
   `just test`, and `just ci` from the release commit.

Monitor structured logs for reconciliation failures, settlement retry
failures, ledger paging failures, instance-lock refusal, and runtime budget
exhaustion. Alert on repeated failures or any open reservation older than its
claim deadline.

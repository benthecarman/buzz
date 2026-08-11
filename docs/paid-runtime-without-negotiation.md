# Paid Agent runtime without a live negotiation

**Status: proposal.** Nothing here is implemented. It describes where the paid
runtime flow in [`docs/nips/AGENT_RUNTIME_PAYMENTS.md`](nips/AGENT_RUNTIME_PAYMENTS.md)
should go, and why the current shape keeps producing outages.

## What is wrong today

Buying runtime is a synchronous negotiation over the relay:

1. The payer publishes an encrypted reservation request (kind `24210`).
2. The Agent answers with a quote, a reservation, or unavailable (kind `24211`).
3. The payer pays the BOLT12 offer named in the quote.
4. The payer asks again, and keeps asking until the Agent sees the credit.

Three consequences follow from that shape.

**A purchase needs three processes online at once.** The payer's app, the
Agent harness, and — because it holds the wallet the payment lands in — the
**owner's desktop**, which mints the deposit on a 15-second reconciler
(`reconcile_agent_runtime_deposits`). An Agent that is restarting, reconnecting,
or rate-limited cannot sell, even though nothing about selling requires it to
think.

**The payer busy-waits.** Each request polls `POST /query` 100 times at 200ms
— 20 seconds — and after payment the client retries the whole request up to 30
more times at 1-second spacing (`useAgentRuntimeCheckout.ts`). The worst case is
minutes of polling that exists only to discover that another machine's timer has
fired.

**The two halves disagree about what the negotiation events are.** The relay
treats `24210`/`24211` as ephemeral: pub/sub only, never stored. Both clients
treat them as stored — the payer publishes over HTTP and then polls a store that
will never hold them. This mismatch is what produced `HTTP 500: ephemeral events
(kind 24210) must not be stored`, and it would have produced a silent 20-second
timeout immediately after that was fixed.

## The Agent already publishes everything a payer needs

Three durable, agent-signed events describe the terms completely:

| Kind    | Carries                                        |
|---------|------------------------------------------------|
| `10101` | the per-runtime-minute rate (replaceable)       |
| `10058` | the BOLT12 offer payment settles against        |
| `10100` | access mode, allowlist, and channels            |

A payer holding those three can decide whether it may invoke the Agent, compute
what 15/30/60 minutes costs, and pay — without asking anyone's permission and
without anyone being awake.

## Proposed model: pay against published terms, settle against the ledger

1. The payer reads `10101`, `10058`, and `10100`, checks it is in the audience,
   and computes `rate × minutes`.
2. The payer zaps the offer, tagging the Agent, the channel, the minutes, and
   **the id of the pricing event it paid against**.
3. The owner's wallet host attests the payment by minting the deposit (`44210`),
   exactly as today.
4. The Agent issues the reservation (`44211`) from available credit when the
   instruction arrives. The relay's claim trigger already makes a reservation
   single-use, so this needs no new enforcement.
5. Settlement (`44212`) is unchanged.

Kinds `24210` and `24211` disappear, along with both poll loops and the
requirement that an Agent be online to sell.

## What must be settled before building it

**Stale prices.** `10101` is replaceable, so "the price when I paid" is only
provable if the payment names the pricing event id. The Agent honors that rate
when it is current or within a grace window, and credits at its current rate
otherwise. Without this, a price change between read and pay is ambiguous.

**Unsolicited credit.** Paying without asking lets a stranger push sats at an
Agent that would never serve them. **There is no refund path in the codebase
today** — no component in `buzz-acp/src/paid_runtime.rs` or the desktop's
`agent_runtime.rs` can return a deposit. Either that path gets built, or the
product states plainly that credit is non-refundable and execution stays gated
by `respond_to`. This is the prerequisite, not a detail.

**Payment attestation.** Only the party that can see the money arrive can
attest it, and that is the owner's wallet host. This proposal shortens the
critical path — no live Agent at purchase time — but does not remove the
owner's machine from it. Removing it means either an Agent-held wallet or
relay-side verification of BOLT12 receipts, both of which change the trust
model and deserve their own decision.

## Alternatives considered

**Move the negotiation to stored kinds and keep polling.** Needs new kind
numbers outside 20000–29999, a migration adding them to the `search_tsv` NULL
list so encrypted quotes are never full-text indexed, and a retention reaper:
the relay does not honor NIP-40 `expiration` for stored events today, so
five-minute quotes would accumulate forever. It buys nothing except keeping a
poll loop.

**Keep the kinds ephemeral and have the payer await the reply on its socket.**
Correct, and smaller than the above — but it preserves the live handshake, so it
preserves the three-process requirement and the offline-Agent failure. Worth
doing as an interim unblock; not worth keeping.

## The invariant that broke, and must hold either way

The HTTP bridge and the WebSocket handler must agree about which kinds are
storable. They did not: the socket routed ephemeral kinds to a fan-out that
never stores, while `POST /events` handed every kind to `ingest_event`. Both now
share `publish_ephemeral_event`. Any future transport must route through the
same seam rather than re-deciding.

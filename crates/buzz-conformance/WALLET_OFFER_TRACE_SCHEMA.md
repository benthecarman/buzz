# Wallet Offer Runtime Trace Schema

Schema version: **2** (`OFFER_TRACE_SCHEMA_VERSION`).

## Modeled seam

This schema binds the signed kind `10058` publication boundary in
`desktop/src-tauri/src/commands/wallet.rs` to
`docs/spec/WalletOfferLifecycle.tla`.

The emitter starts before the first relay request. It records the terminal
result for each target relay and then records completion or a hard-error abort.

## Abstract state

| Field | Spec variable | Meaning |
|---|---|---|
| `phase` | `phase` | Idle, announcing, or withdrawing |
| `active_offer` | `activeOffer` | Last completed offer for an identity |
| `pending_offer` | `pendingOffer` | Offer in the current fan-out |
| `target_relays` | `targets` | Exact deduplicated relay target set |
| `attempted_relays` | `attempted` | Targets with a terminal result |
| `accepted_relays` | `accepted` | Attempted targets that accepted |

Identity, offer, and relay labels are domain-separated SHA-256 prefixes. The
trace contains no private key, raw offer, relay URL, event body, signature, or
authentication header.

## Actions

| Runtime action | Spec action | Critical |
|---|---|---|
| `begin_announcement` | `BeginAnnouncement` | Yes |
| `begin_withdrawal` | `BeginWithdrawal` | Yes |
| `relay_result` | `RelayResult` | Yes |
| `finish_announcement` | `FinishAnnouncement` | Yes |
| `finish_withdrawal` | `FinishWithdrawal` | Yes |
| `abort` | `Abort` | Yes |
| `impl_bug` | Coverage breach | Yes |

The begin actions retain the raw target list in addition to the projected set.
The checker rejects duplicates before set conversion. They also retain the
actual event kind and author-match boolean, so projection cannot hide a wrong
kind or wrong signer.
Announcements also record whether the offer issuer matches the active user's
wallet. The checker rejects offers derived from an agent wallet.

## Failure policy

The checker fails for these conditions:

- The event has a wrong kind or author.
- The offer issuer is not the active user's wallet.
- The target list is empty or contains duplicates.
- Different identities use the same offer.
- A result names a non-target relay or repeats a target.
- The operation completes before every target has a result.
- The trace has a state mismatch, malformed action, missing action, or `impl_bug`.

## Examples

A valid announcement starts, records one result per target, and finishes. A
withdrawal uses the same sequence with `begin_withdrawal` and
`finish_withdrawal`. See `tests/fixtures/wallet_offer/` for accepted and
rejected JSONL traces.

## CI command

```sh
just wallet-formal
```

This runs TLC for both wallet models and replays the offer fixtures and
property-generated traces through the independent checker.

## Limits

Runtime checking covers executed publication paths only. It does not prove
Lexe settles an offer to a particular wallet, creates cryptographically
distinct offers, Nostr signatures are secure,
or a relay that accepted an event will retain it. Offer uniqueness is checked
for opaque offers observed together in one process trace. Secondary HTTP retry
attempts are projected as one terminal result for that relay.

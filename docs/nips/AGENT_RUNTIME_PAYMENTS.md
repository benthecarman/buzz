# Paid Agent invocation access

Buzz uses one settled BOLT12 zap as proof of paid Agent access. A payment
opens a fixed five-minute invocation window. The payment does not create
credits, runtime reservations, or metered usage.

## Events

| Kind | Author | Purpose |
|---|---|---|
| `10058` | Agent | Current BOLT12 offer |
| `10101` | Agent | Current price and invocation window |
| `9737` | Payer | Signed zap intent, embedded in kind `9736` |
| `9736` | Payer | Settled zap and access proof |
| Message kind | Payer | Agent instruction that cites the zap |

Kind `10101` is a replaceable event. Enabled pricing has this content:

```json
{
  "version": 2,
  "enabled": true,
  "price_sats": 255,
  "invocation_window_seconds": 300
}
```

`price_sats` must be a positive whole-satoshi amount.
`invocation_window_seconds` must be `300`. Disabled pricing contains only
`version` and `enabled: false`.

## Payment flow

1. The payer reads the Agent's kinds `10058` and `10101`.
2. The payer creates a signed kind `9737` zap intent.
3. The intent targets the exact kind `10101` event with these tags:
   `p` for the Agent, `e` for pricing, and `h` for the channel.
4. The intent also contains the amount in millisatoshis and the signed offer
   event. It does not contain a `k` tag. The `e` tag identifies the kind.
5. The wallet pays the Agent's BOLT12 offer.
6. After settlement, the payer publishes kind `9736`. It embeds the exact
   kind `9737` intent and the BOLT12 payer proof.

The desktop attaches the settled zap to each instruction:

```json
["agent_runtime", "<agent-pubkey>", "<kind-9736-event-id>"]
```

The same zap can start more than one invocation during its five-minute
window. The Agent does not consume the zap.

## Agent admission

The Agent checks the proof when it receives an instruction. It verifies all
of these conditions:

- The instruction author also authored the zap and its embedded intent.
- The zap has the correct `p`, `e`, and `h` tags.
- The `e` tag names a signed kind `10101` event from this Agent.
- The amount equals `price_sats` from that pricing event.
- The embedded kind `10058` offer is signed by this Agent and is canonical.
- The instruction timestamp is not before the zap timestamp.
- The instruction timestamp is at most 300 seconds after the zap timestamp.
- The payer is in the Agent's permitted audience.

The timestamp check applies only when the invocation starts. A request can
continue after the five-minute window ends.

If the instruction starts after the window, the desktop asks for a new
payment. The desktop does not ask the user to select a pack or duration.

Paid access is not available in direct messages. Owners do not pay their own
Agents.

## Current proof limit

The wallet provider does not yet expose the final BOLT12 payer proof. Desktop
therefore publishes the exact value `placeholder` after its wallet reports a
settled payment.

This marker proves only that the payer signed the Nostr event. It does not
give the Agent cryptographic proof of Lightning settlement. A malicious payer
can forge it. This temporary behavior is suitable only for the current demo.
Remove support for the marker when the wallet provider exposes real payer
proofs.

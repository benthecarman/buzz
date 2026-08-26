# Draft BOLT12 zap support

Buzz's experimental Bitcoin wallet implements the candidate event kinds from
the BOLT12 zap proposal:

- `9736`: settled zap event, admitted as a global profile write or as a
  channel-scoped message write when it carries an `h` tag;
- `9737`: signed zap intent, embedded in `9736` and rejected if broadcast; and
- `10058`: recipient offer announcement, admitted as global replaceable user
  state.

The relay validates the complete zap proof chain before it stores kind `9736`.
It verifies the outer event, embedded intent, recipient offer, BOLT12 payer
proof, amount, signer, recipient, target, and channel bindings. The relay also
stores the payment hash with the event in one transaction. It rejects a second
zap event for the same payment hash in the same community. Clients can render
and count stored zaps in the same event path as reactions.

After settlement, Desktop requests an `lnp` payer proof from Lexe. The proof
includes the amount, payment hash, and signed zap-intent note. Kind `9736` uses
the normal current event time. The relay uses rust-lightning to make sure that
the proof is valid before it stores the event.

## Agent payments

Agents request outgoing BOLT12 payments with NIP-47 kind `23194` and the
optional NWC-321 `pay` method. The agent first signs an unbroadcast kind `9737`
intent. It sends the intent ID as the BOLT12 payer note. Buzz Desktop decrypts
the request and makes sure that the author is an approved NWC client. Then the
wallet owner approves or denies the request. An approval uses the durable send
state machine. Desktop returns the result in an encrypted kind `23195`
response.

Kinds `23194` and `23195` are ephemeral. The agent and Buzz Desktop must both
be connected to the same active community relay while the approval is pending;
Buzz does not treat the relay as a durable payment-request queue.

Buzz uses the agent identity as the NWC client key. It uses the owner identity
as the wallet-service key. This pair is unique for each agent connection. It
also works with relay membership authentication. It does not give the ideal
NIP-47 unlinkability. A future relay delegation mechanism can use opaque
connection keys.

The factory does not send an NWC URI or a separate client secret. The agent
already has its derived identity key. Both sides already have the community
relay and wallet-service pubkey. Desktop keeps the normal payment approval for
each request.

For a hosted agent, Desktop first verifies the factory request against the
buyer's paid zap and the advertised plan. Desktop signs the NIP-OA owner
attestation only after this check. It then stores the agent as a normal NWC
client for that community. The normal approval prompt remains active for each
payment when the agent uses manual approval. An owner-selected agent budget can
approve payments automatically while enough budget remains. The factory never
receives the wallet seed.

Desktop creates a stable BOLT12 offer for each hosted agent. The buyer includes
this offer in the signed ownership response. The agent then signs its own kind
`10058` announcement. Payments to this offer enter the buyer's wallet.

After settlement, the response includes the Lightning preimage. It also
includes the BOLT12 payer proof when the provider exposes one. An on-chain
payment includes its Bitcoin transaction ID. For a zap, the agent then
publishes kind `9736` with the payer proof.

The desktop subscribes to kind `9736` events that tag the user or a managed
agent. It extracts display fields from relay-validated events before it adds a
local Inbox and zap-history record. This display path does not depend on the
recipient wallet being enabled or synchronized. A relay-and-recipient cursor
pages stored proofs after an offline period, with a five-second overlap and
event-ID deduplication at the local history boundary. The relay parses `lnp`
proofs with rust-lightning. This validates the TLV structure, payment preimage,
invoice signature, and payer signature.

Zap consumption is relay-driven. Message badges, history, notifications, and
hosted-agent purchases use kind `9736` events and their embedded signed data.
A hosted-agent purchase targets a normal channel plan message. The intent and
proof include the plan `e` tag, host `p` tag, and source channel `h` tag.

The host verifies the payer proof, amount, recipient, and signed intent. The
host does not query the receiving wallet.

For a new hosted agent, the buyer and factory derive the same identity. They
use NIP-44 ECDH and HKDF-SHA256. The context binds the factory, buyer, plan,
and signed zap intent. Desktop verifies the derived public key and lease ID
before it signs the NIP-OA owner attestation.

The HKDF context is the domain `buzz-agent-factory:agent-key:v1`, followed by
the raw factory pubkey, buyer pubkey, plan event ID, and zap intent event ID.
There is no counter. The lease ID is UUIDv5 over the derived agent pubkey with
namespace `5d63a291-e458-4df2-9bd9-ba47b9f06a38`.

## Units

All wallet amounts are whole satoshis end-to-end. The UI labels and displays
them with the ₿ symbol per
[BIP-177](https://github.com/bitcoin/bips/blob/master/bip-0177.mediawiki),
which redefines ₿ as the base unit (1 ₿ = 1 satoshi). This is deliberate, not
a units bug: inputs, toasts, and history all mean satoshis when they show ₿.

## Offer announcements

Kind `10058` is a replaceable event. Payers treat the newest `10058` per
author as authoritative, including an empty announcement with no `offer` tag,
which withdraws the offer. A relay that missed the withdrawal and still serves
an older announcement must not resurrect the withdrawn offer.

The desktop wallet persists its active offer because Lexe 0.1.20 cannot recover
an existing offer. The `create_offer` API never invalidates prior offers.
Without persistence, each app restart creates and publishes a new offer. The
old offers remain payable.

The POC protocol reference is the latest proposed
[NIP-B1 BOLT12 zaps draft](https://github.com/benthecarman/nips/blob/bolt12-zaps/B1.md).
The candidate kind numbers are not final until the proposal is accepted.

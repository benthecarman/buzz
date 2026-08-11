# Draft BOLT12 zap support

Buzz's experimental Bitcoin wallet implements the candidate event kinds from
the BOLT12 zap proposal:

- `9736`: settled zap event, admitted as a global profile write or as a
  channel-scoped message write when it carries an `h` tag;
- `9737`: signed zap intent, embedded in `9736` and rejected if broadcast; and
- `10058`: recipient offer announcement, admitted as global replaceable user
  state.

The relay only authenticates, authorizes, stores, replaces, and queries these
events. It does not validate the embedded intent, recipient offer, BOLT12 payer
proof, amount, target, or payment hash. Clients must validate that proof chain
before rendering or counting a zap.

Lexe 0.1.20 does not expose the settled `lnp` payer proof required by the
proposal. During this experimental phase, Desktop publishes the literal value
`placeholder` in the kind `9736` `proof` tag after the payment settles. Buzz
clients treat that exact temporary marker as a valid proof everywhere a zap is
displayed or counted. They also accept a payer proof that rust-lightning can
parse and cryptographically verify. The temporary marker does not provide
NIP-B1's cryptographic settlement guarantee and must be removed when the wallet
provider exposes real payer proofs.

## Managed-agent payments

Managed agents request outgoing BOLT12 payments with NIP-47 kind `23194` and
the optional NWC-321 `pay` method. The agent signs an unbroadcast kind `9737`
intent first and sends its id as the BOLT12 payer note. Buzz Desktop validates
and decrypts the request, confirms that its author is a managed agent, and asks
the wallet owner to approve or deny it. Approval enters the same durable
generic-send state machine as a user-initiated wallet send. Desktop returns the
result in an encrypted kind `23195` response.

Kinds `23194` and `23195` are ephemeral. The agent and Buzz Desktop must both
be connected to the same active community relay while the approval is pending;
Buzz does not treat the relay as a durable payment-request queue.

Buzz uses the agent identity as the NWC client key and the owner identity as
the wallet-service key in this first community-relay implementation. These are
unique per agent connection and compatible with the relay's membership
authentication, but they do not provide NIP-47's ideal unlinkability. A future
relay delegation mechanism can move NWC onto opaque connection keys.

Because Lexe 0.1.20 does not expose `payer_proof`, the response uses the same
literal `placeholder` marker after settlement. The agent then publishes kind
`9736` with that marker.

The desktop subscribes to kind `9736` events that tag the user or a managed
agent. It validates the signed Nostr envelope, intent, offer announcement, and
proof directly from the relay event before it adds a local Inbox and
zap-history record. This display path does not depend on the recipient wallet
being enabled or synchronized. A relay-and-recipient cursor pages stored proofs
after an offline period, with a five-second overlap and event-ID deduplication
at the local history boundary. Real `lnp` proofs are parsed with rust-lightning,
which validates their TLV structure, payment preimage, invoice signature, and
payer signature. During the temporary compatibility period, the exact
`placeholder` marker is valid for the same UI surfaces and totals.

Zap consumption is relay-driven. Message badges, history, notifications, and
paid Agent access use validated kind `9736` events and their embedded signed
data. Paid Agent access targets the exact kind `10101` pricing event. The zap
opens a fixed invocation window; it does not create a credit balance or a
runtime ledger. The zap flow contacts the wallet provider only to send a
payment or reconcile a payment whose result is unknown. Proof publication and
replay use the saved payment attempt and the relay. They do not need another
wallet lookup.

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

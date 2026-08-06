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

The desktop wallet does not currently publish kind `9736`. Lexe 0.1.20 does
not expose the settled `lnp` payer proof required by the proposal, so Buzz
keeps the signed intent and payment result local until a valid proof is
available.

The desktop subscribes to kind `9736` events that tag the user or a managed
agent. It validates the signed Nostr envelope, intent, and offer announcement,
then correlates the intent with settled inbound wallet history before it adds a
local Inbox and zap-history record. This notification path validates the `lnp`
Bech32 encoding but does not yet validate its payer-proof cryptography, so these
records must not be used for public zap totals.

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

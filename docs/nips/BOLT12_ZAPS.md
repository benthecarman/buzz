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

The desktop wallet does not currently publish kind `9736`. Lexe 0.1.19 does
not expose the settled `lnp` payer proof required by the proposal, so Buzz
keeps the signed intent and payment result local until a valid proof is
available.

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

The desktop wallet persists its active offer locally because Lexe 0.1.19 has
no API to recover an existing offer and `create_offer` never invalidates prior
ones; without persistence every app restart would mint and publish a fresh
offer while the old ones stay payable.

The POC protocol reference is the proposed
[BOLT12 zaps NIP](https://github.com/benthecarman/nips/blob/035b3cf4d5fadb808031b94f2277ba98dc94e9ac/B1.md).
The candidate kind numbers are not final until the proposal is accepted.

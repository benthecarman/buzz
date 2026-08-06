import assert from "node:assert/strict";
import test from "node:test";

import { finalizeEvent, generateSecretKey } from "nostr-tools/pure";

import { parseTaggedZapEvent, zapSubscriptionFilter } from "./zapEvents.ts";

const BECH32_ALPHABET = "qpzry9x8gf2tvdw0s3jn54khce6mua7l";

function bech32(prefix, words = [1]) {
  const values = [
    ...Array.from(prefix, (character) => character.charCodeAt(0) >> 5),
    0,
    ...Array.from(prefix, (character) => character.charCodeAt(0) & 31),
    ...words,
    0,
    0,
    0,
    0,
    0,
    0,
  ];
  const generators = [
    0x3b6a57b2, 0x26508e6d, 0x1ea119fa, 0x3d4233dd, 0x2a1462b3,
  ];
  let checksum = 1;
  for (const value of values) {
    const top = checksum >>> 25;
    checksum = ((checksum & 0x1ffffff) << 5) ^ value;
    for (let index = 0; index < generators.length; index += 1) {
      if ((top >>> index) & 1) checksum ^= generators[index];
    }
  }
  checksum ^= 1;
  const checksumWords = Array.from(
    { length: 6 },
    (_, index) => (checksum >>> (5 * (5 - index))) & 31,
  );
  return `${prefix}1${[...words, ...checksumWords]
    .map((word) => BECH32_ALPHABET[word])
    .join("")}`;
}

function signedZapFixture() {
  const recipientSecret = generateSecretKey();
  const payerSecret = generateSecretKey();
  const offerEvent = finalizeEvent(
    {
      kind: 10058,
      created_at: 100,
      content: "",
      tags: [["offer", bech32("lno")]],
    },
    recipientSecret,
  );
  const intent = finalizeEvent(
    {
      kind: 9737,
      created_at: 101,
      content: "nice work",
      tags: [
        ["p", offerEvent.pubkey],
        ["amount", "21000"],
        ["offer_event", JSON.stringify(offerEvent)],
        ["zap_id", "00112233445566778899aabbccddeeff"],
      ],
    },
    payerSecret,
  );
  const zap = finalizeEvent(
    {
      kind: 9736,
      created_at: 102,
      content: intent.content,
      tags: [
        ["description", JSON.stringify(intent)],
        ["p", offerEvent.pubkey],
        ["P", intent.pubkey],
        ["amount", "21000"],
        ["offer_event", JSON.stringify(offerEvent)],
        ["proof", bech32("lnp")],
      ],
    },
    payerSecret,
  );
  return { intent, offerEvent, payerSecret, zap };
}

function resignZap(zap, payerSecret, tags) {
  return finalizeEvent(
    {
      kind: zap.kind,
      created_at: zap.created_at,
      content: zap.content,
      tags,
    },
    payerSecret,
  );
}

test("valid tagged zap envelope exposes settlement correlation fields", () => {
  const { intent, offerEvent, zap } = signedZapFixture();
  assert.deepEqual(parseTaggedZapEvent(zap, new Set([offerEvent.pubkey])), {
    amount: 21,
    comment: "nice work",
    intentEventId: intent.id,
    recipientPubkey: offerEvent.pubkey,
    targetEventId: null,
  });
});

test("literal placeholder payer proof is accepted temporarily", () => {
  const { offerEvent, payerSecret, zap } = signedZapFixture();
  const placeholder = resignZap(
    zap,
    payerSecret,
    zap.tags.map((tag) =>
      tag[0] === "proof" ? ["proof", "placeholder"] : tag,
    ),
  );
  assert.ok(parseTaggedZapEvent(placeholder, new Set([offerEvent.pubkey])));
});

test("only the exact placeholder payer proof marker is accepted", () => {
  const { offerEvent, payerSecret, zap } = signedZapFixture();
  const placeholder = resignZap(
    zap,
    payerSecret,
    zap.tags.map((tag) =>
      tag[0] === "proof" ? ["proof", "Placeholder"] : tag,
    ),
  );
  assert.equal(
    parseTaggedZapEvent(placeholder, new Set([offerEvent.pubkey])),
    null,
  );
});

test("tagged zap for another recipient is ignored", () => {
  const { zap } = signedZapFixture();
  assert.equal(parseTaggedZapEvent(zap, new Set(["f".repeat(64)])), null);
});

test("tagged zap rejects a malformed payer proof encoding", () => {
  const { offerEvent, payerSecret, zap } = signedZapFixture();
  const tags = zap.tags.map((tag) =>
    tag[0] === "proof" ? ["proof", "lnp1not-a-proof"] : tag,
  );
  const malformed = resignZap(zap, payerSecret, tags);
  assert.equal(
    parseTaggedZapEvent(malformed, new Set([offerEvent.pubkey])),
    null,
  );
});

test("tagged zap rejects duplicate recipient tags", () => {
  const { offerEvent, payerSecret, zap } = signedZapFixture();
  const duplicate = resignZap(zap, payerSecret, [
    ...zap.tags,
    ["p", offerEvent.pubkey],
  ]);
  assert.equal(
    parseTaggedZapEvent(duplicate, new Set([offerEvent.pubkey])),
    null,
  );
});

test("zap subscription covers the owner and agents once", () => {
  assert.deepEqual(zapSubscriptionFilter(["AA", "bb", "aa"], 123), {
    kinds: [9736],
    "#p": ["aa", "bb"],
    limit: 50,
    since: 123,
  });
});

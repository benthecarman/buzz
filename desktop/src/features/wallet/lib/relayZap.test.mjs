import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";

import { parseRelayZapEvent, payerProofPaymentHash } from "./relayZap.ts";

const PAYMENT_HASH = "ab".repeat(32);
const BECH32_CHARSET = "qpzry9x8gf2tvdw0s3jn54khce6mua7l";
const REAL_PAYMENT_HASH =
  "3df0d67cc26c4b5360fb216ea047abf8fe2426e93a2ee51b3124c5f6c9b03698";

function realPayerProof() {
  const rustTests = readFileSync(
    new URL("../../../../src-tauri/src/wallet/zap/tests.rs", import.meta.url),
    "utf8",
  );
  return rustTests.match(/const TIMESTAMPED_PROOF: &str = "([^"]+)";/)?.[1];
}

function encodeBolt12Data(bytes) {
  let accumulator = 0;
  let bitCount = 0;
  const words = [];
  for (const byte of bytes) {
    accumulator = (accumulator << 8) | byte;
    bitCount += 8;
    while (bitCount >= 5) {
      bitCount -= 5;
      words.push((accumulator >> bitCount) & 31);
    }
    accumulator &= (1 << bitCount) - 1;
  }
  if (bitCount > 0) words.push((accumulator << (5 - bitCount)) & 31);
  return `lnp1${words.map((word) => BECH32_CHARSET[word]).join("")}`;
}

function payerProof(paymentHash = PAYMENT_HASH) {
  const hash = Uint8Array.from(
    paymentHash.match(/.{2}/g).map((byte) => Number.parseInt(byte, 16)),
  );
  const bytes = Uint8Array.from([168, hash.length, ...hash]);
  return encodeBolt12Data(bytes);
}

function zapEvent(overrides = {}) {
  const intent = {
    id: "intent-id",
    content: "nice work",
  };
  return {
    id: "zap-id",
    pubkey: "payer",
    created_at: 123,
    kind: 9736,
    tags: [
      ["p", "RECIPIENT"],
      ["amount", "21000"],
      ["description", JSON.stringify(intent)],
      ["proof", payerProof()],
      ["e", "target-id"],
      ["h", "channel-id"],
    ],
    content: "nice work",
    sig: "signature",
    ...overrides,
  };
}

test("extracts a relay-validated zap synchronously", () => {
  assert.deepEqual(parseRelayZapEvent(zapEvent()), {
    eventId: "zap-id",
    amount: 21,
    comment: "nice work",
    intentEventId: "intent-id",
    recipientPubkey: "recipient",
    paymentHash: PAYMENT_HASH,
    targetEventId: "target-id",
    targetEventKind: null,
    channelId: "channel-id",
    leaseId: null,
  });
});

test("extracts a payment hash from the payer-proof TLV stream", () => {
  assert.equal(payerProofPaymentHash(payerProof()), PAYMENT_HASH);
  assert.equal(
    payerProofPaymentHash(realPayerProof() ?? ""),
    REAL_PAYMENT_HASH,
  );
  assert.equal(payerProofPaymentHash("not-a-proof"), null);
});

test("ignores malformed display data defensively", () => {
  assert.equal(
    parseRelayZapEvent(
      zapEvent({
        tags: [
          ["p", "recipient"],
          ["amount", "bad"],
          ["description", "{}"],
        ],
      }),
    ),
    null,
  );
});

test("ignores non-zap events", () => {
  assert.equal(parseRelayZapEvent(zapEvent({ kind: 7 })), null);
});

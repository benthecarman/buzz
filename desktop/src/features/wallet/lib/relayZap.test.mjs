import assert from "node:assert/strict";
import test from "node:test";

import { parseRelayZapEvent } from "./relayZap.ts";

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
    targetEventId: "target-id",
    channelId: "channel-id",
  });
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

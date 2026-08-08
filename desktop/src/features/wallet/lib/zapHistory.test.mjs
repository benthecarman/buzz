import assert from "node:assert/strict";
import test from "node:test";

import {
  addZapHistoryItem,
  parseZapHistory,
  zapHistoryFeedItems,
} from "./zapHistory.ts";

function zap(overrides = {}) {
  return {
    amount: 21,
    comment: "nice work",
    createdAt: 123,
    eventId: "event-a",
    intentEventId: "intent-a",
    payerPubkey: "payer",
    recipientName: "Agent Smith",
    recipientPubkey: "recipient",
    targetEventId: null,
    ...overrides,
  };
}

test("zap history deduplicates events and keeps newest first", () => {
  const existing = [zap()];
  assert.deepEqual(addZapHistoryItem(existing, zap()), {
    didAdd: false,
    items: existing,
  });
  assert.deepEqual(
    addZapHistoryItem(existing, zap({ createdAt: 456, eventId: "event-b" })),
    {
      didAdd: true,
      items: [zap({ createdAt: 456, eventId: "event-b" }), zap()],
    },
  );
});

test("zap history parser drops malformed records", () => {
  assert.deepEqual(parseZapHistory(JSON.stringify([{}, zap()])), [zap()]);
  assert.deepEqual(parseZapHistory("not-json"), []);
});

test("zap history projects to Inbox activity", () => {
  assert.deepEqual(zapHistoryFeedItems([zap()]), [
    {
      id: "event-a",
      kind: 9736,
      pubkey: "payer",
      content: "₿ 21 · nice work",
      createdAt: 123,
      channelId: null,
      channelName: "",
      tags: [
        ["p", "recipient"],
        ["amount", "21000"],
        ["buzz_recipient_name", "Agent Smith"],
      ],
      category: "activity",
    },
  ]);
});

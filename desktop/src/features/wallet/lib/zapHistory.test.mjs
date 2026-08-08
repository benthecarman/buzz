import assert from "node:assert/strict";
import test from "node:test";

import {
  addZapHistoryItem,
  parseZapHistory,
  persistZapHistoryItem,
  readZapHistory,
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

test("zap history storage is relay-scoped and reports write failures", () => {
  const originalWindow = globalThis.window;
  const originalLocalStorage = globalThis.localStorage;
  const originalCustomEvent = globalThis.CustomEvent;
  const rows = new Map();
  globalThis.window = { dispatchEvent() {} };
  globalThis.CustomEvent = class {
    constructor(_name, init) {
      this.detail = init?.detail;
    }
  };
  globalThis.localStorage = {
    getItem(key) {
      return rows.get(key) ?? null;
    },
    setItem(key, value) {
      rows.set(key, value);
    },
    removeItem(key) {
      rows.delete(key);
    },
  };

  try {
    assert.equal(
      persistZapHistoryItem("owner", "wss://relay-a.example", zap()),
      "added",
    );
    assert.equal(
      persistZapHistoryItem("owner", "wss://relay-a.example", zap()),
      "duplicate",
    );
    assert.deepEqual(readZapHistory("owner", "wss://relay-a.example"), [zap()]);
    assert.deepEqual(readZapHistory("owner", "wss://relay-b.example"), []);

    globalThis.localStorage.setItem = () => {
      throw new Error("quota exceeded");
    };
    assert.equal(
      persistZapHistoryItem(
        "owner",
        "wss://relay-a.example",
        zap({ eventId: "event-b" }),
      ),
      "failed",
    );
  } finally {
    if (originalWindow === undefined) delete globalThis.window;
    else globalThis.window = originalWindow;
    if (originalLocalStorage === undefined) delete globalThis.localStorage;
    else globalThis.localStorage = originalLocalStorage;
    if (originalCustomEvent === undefined) delete globalThis.CustomEvent;
    else globalThis.CustomEvent = originalCustomEvent;
  }
});

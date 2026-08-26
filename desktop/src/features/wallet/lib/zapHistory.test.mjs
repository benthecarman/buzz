import assert from "node:assert/strict";
import test from "node:test";

import {
  addZapHistoryItem,
  parseZapHistory,
  persistZapHistoryItem,
  readZapHistory,
} from "./zapHistory.ts";

function zap(overrides = {}) {
  return {
    amount: 21,
    channelId: null,
    comment: "nice work",
    createdAt: 123,
    eventId: "event-a",
    intentEventId: "intent-a",
    leaseId: null,
    paymentHash: null,
    payerPubkey: "payer",
    recipientName: "Agent Smith",
    recipientPubkey: "recipient",
    targetEventId: null,
    targetEventKind: null,
    ...overrides,
  };
}

test("zap history deduplicates events and keeps newest first", () => {
  const existing = [zap()];
  assert.deepEqual(addZapHistoryItem(existing, zap()), {
    didAdd: false,
    didUpdate: false,
    items: existing,
  });
  assert.deepEqual(
    addZapHistoryItem(existing, zap({ createdAt: 456, eventId: "event-b" })),
    {
      didAdd: true,
      didUpdate: false,
      items: [zap({ createdAt: 456, eventId: "event-b" }), zap()],
    },
  );
});

test("zap history parser drops malformed records", () => {
  assert.deepEqual(parseZapHistory(JSON.stringify([{}, zap()])), [zap()]);
  assert.deepEqual(
    parseZapHistory(JSON.stringify([zap({ paymentHash: "not-a-hash" })])),
    [zap()],
  );
  assert.deepEqual(parseZapHistory("not-json"), []);
});

test("zap history parser migrates records without target context", () => {
  const legacy = { ...zap() };
  delete legacy.channelId;
  delete legacy.leaseId;
  delete legacy.paymentHash;
  delete legacy.targetEventKind;
  assert.deepEqual(parseZapHistory(JSON.stringify([legacy])), [zap()]);
});

test("zap history enriches an existing proof with its payment hash", () => {
  const paymentHash = "ab".repeat(32);
  assert.deepEqual(addZapHistoryItem([zap()], zap({ paymentHash })), {
    didAdd: false,
    didUpdate: true,
    items: [zap({ paymentHash })],
  });
});

test("zap history enriches an existing proof with its recipient name", () => {
  assert.deepEqual(
    addZapHistoryItem(
      [zap({ recipientName: "" })],
      zap({ recipientName: "Remote Agent" }),
    ),
    {
      didAdd: false,
      didUpdate: true,
      items: [zap({ recipientName: "Remote Agent" })],
    },
  );
});

test("zap history storage is relay-scoped and reports write failures", () => {
  const originalWindow = globalThis.window;
  const originalLocalStorage = globalThis.localStorage;
  const rows = new Map();
  globalThis.window = {};
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
    assert.equal(
      persistZapHistoryItem(
        "owner",
        "wss://relay-a.example",
        zap({ paymentHash: "ab".repeat(32) }),
      ),
      "updated",
    );
    assert.deepEqual(readZapHistory("owner", "wss://relay-a.example"), [
      zap({ paymentHash: "ab".repeat(32) }),
    ]);
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
  }
});

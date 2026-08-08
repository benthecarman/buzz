import assert from "node:assert/strict";
import test from "node:test";

import {
  clearAgentRuntimeCheckout,
  loadAgentRuntimeCheckout,
  saveAgentRuntimeCheckout,
} from "./agentRuntimeCheckoutStorage.ts";

class MemoryStorage {
  #values = new Map();

  get length() {
    return this.#values.size;
  }

  clear() {
    this.#values.clear();
  }

  getItem(key) {
    return this.#values.get(key) ?? null;
  }

  key(index) {
    return [...this.#values.keys()][index] ?? null;
  }

  removeItem(key) {
    this.#values.delete(key);
  }

  setItem(key, value) {
    this.#values.set(key, String(value));
  }
}

const row = {
  pubkey: "a".repeat(64),
  name: "Metered Agent",
  rateSats: 20,
  availableMs: 0,
  requestId: "request-id",
  zapIdempotencyKey: "zap-idempotency-key",
  quoteEventJson: '{"id":"signed-quote"}',
  paymentSent: true,
  reservationTag: ["agent_runtime", "a".repeat(64), "reservation-id"],
};

test("checkout identities and successful partial work survive a renderer restart", () => {
  const storage = new MemoryStorage();
  saveAgentRuntimeCheckout(
    "community-a:channel-a",
    { channelId: "channel-a", capMinutes: 30, rows: [row] },
    storage,
  );

  const restored = loadAgentRuntimeCheckout("community-a:channel-a", storage);
  assert.equal(restored?.capMinutes, 30);
  assert.equal(restored?.rows[0]?.requestId, "request-id");
  assert.equal(restored?.rows[0]?.zapIdempotencyKey, "zap-idempotency-key");
  assert.equal(restored?.rows[0]?.quoteEventJson, row.quoteEventJson);
  assert.equal(restored?.rows[0]?.paymentSent, true);
  assert.deepEqual(restored?.rows[0]?.reservationTag, row.reservationTag);
});

test("checkout state is cleared only after the caller confirms message send", () => {
  const storage = new MemoryStorage();
  saveAgentRuntimeCheckout(
    "community-a:channel-a",
    { channelId: "channel-a", capMinutes: 15, rows: [row] },
    storage,
  );
  clearAgentRuntimeCheckout("community-a:channel-a", storage);
  assert.equal(
    loadAgentRuntimeCheckout("community-a:channel-a", storage),
    null,
  );
});

test("malformed durable checkout data fails closed", () => {
  const storage = new MemoryStorage();
  storage.setItem(
    "buzz.agent-runtime-checkout.v1:community-a:channel-a",
    JSON.stringify({ version: 1, channelId: "channel-a", capMinutes: 45 }),
  );
  assert.equal(
    loadAgentRuntimeCheckout("community-a:channel-a", storage),
    null,
  );
  assert.equal(storage.length, 0);
});

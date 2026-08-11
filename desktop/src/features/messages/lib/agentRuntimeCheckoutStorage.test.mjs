import assert from "node:assert/strict";
import test from "node:test";

import {
  activeStoredRuntimeZap,
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
  name: "Paid Agent",
  ownerPubkey: "c".repeat(64),
  priceSats: 255,
  invocationWindowSeconds: 300,
  zapIdempotencyKey: "zap-idempotency-key",
  zapEventId: "b".repeat(64),
  validUntilSeconds: 1_800,
};

test("checkout payment state survives a renderer restart", () => {
  const storage = new MemoryStorage();
  saveAgentRuntimeCheckout(
    "community-a:channel-a",
    { channelId: "channel-a", rows: [row] },
    storage,
  );
  const restored = loadAgentRuntimeCheckout("community-a:channel-a", storage);
  assert.equal(restored?.rows[0]?.zapIdempotencyKey, "zap-idempotency-key");
  assert.equal(restored?.rows[0]?.zapEventId, "b".repeat(64));
});

test("a settled zap remains reusable until its access window ends", () => {
  const storage = new MemoryStorage();
  saveAgentRuntimeCheckout(
    "community-a:channel-a",
    { channelId: "channel-a", rows: [row] },
    storage,
  );
  const restored = loadAgentRuntimeCheckout("community-a:channel-a", storage);
  assert.equal(
    activeStoredRuntimeZap(restored?.rows[0], 1_500),
    "b".repeat(64),
  );
  assert.equal(
    activeStoredRuntimeZap(restored?.rows[0], 1_800),
    "b".repeat(64),
  );
  assert.equal(activeStoredRuntimeZap(restored?.rows[0], 1_801), null);

  clearAgentRuntimeCheckout("community-a:channel-a", storage);
  assert.equal(
    loadAgentRuntimeCheckout("community-a:channel-a", storage),
    null,
  );
});

test("malformed checkout data fails closed", () => {
  const storage = new MemoryStorage();
  storage.setItem(
    "buzz.agent-runtime-checkout.v4:community-a:channel-a",
    JSON.stringify({ version: 4, channelId: "channel-a", rows: [{}] }),
  );
  assert.equal(
    loadAgentRuntimeCheckout("community-a:channel-a", storage),
    null,
  );
});

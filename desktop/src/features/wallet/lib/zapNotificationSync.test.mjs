import assert from "node:assert/strict";
import test from "node:test";

import {
  buildZapCatchupFilter,
  fetchZapCatchupEvents,
  parseZapSyncCursor,
  zapCatchupProgress,
  zapSyncCursorStorageKey,
} from "./zapNotificationSync.ts";

function relayEvent(id, createdAt) {
  return {
    id,
    pubkey: "payer",
    created_at: createdAt,
    kind: 9736,
    tags: [],
    content: "",
    sig: "signature",
  };
}

test("zap catch-up scopes one recipient with a cursor overlap", () => {
  assert.deepEqual(buildZapCatchupFilter("AA", 100, 200), {
    kinds: [9736],
    "#p": ["aa"],
    limit: 500,
    since: 95,
    until: 200,
  });
});

test("zap catch-up pages backward, deduplicates, and returns oldest first", async () => {
  const firstPage = Array.from({ length: 500 }, (_, index) =>
    relayEvent(`new-${index}`, 1_000 + index),
  );
  const overlap = firstPage[0];
  const filters = [];
  const events = await fetchZapCatchupEvents({
    recipientPubkey: "recipient",
    since: 500,
    until: 2_000,
    fetchPage: async (filter) => {
      filters.push(filter);
      return filters.length === 1
        ? firstPage
        : [relayEvent("old", 900), overlap];
    },
  });

  assert.equal(filters.length, 2);
  assert.equal(filters[1].until, 1_000);
  assert.equal(events.length, 501);
  assert.equal(events[0].id, "old");
  assert.equal(events.at(-1).id, "new-499");
});

test("zap catch-up processes later proofs without skipping an unresolved proof", () => {
  assert.deepEqual(
    zapCatchupProgress(50, [
      { createdAt: 80, status: "processed" },
      { createdAt: 100, status: "retry" },
      { createdAt: 120, status: "processed" },
    ]),
    { cursor: 100, hasPending: true },
  );
  assert.deepEqual(
    zapCatchupProgress(50, [
      { createdAt: 80, status: "processed" },
      { createdAt: 120, status: "processed" },
    ]),
    { cursor: 120, hasPending: false },
  );
});

test("zap sync cursor is validated and scoped by relay and recipient", () => {
  assert.equal(parseZapSyncCursor(null), 0);
  assert.equal(parseZapSyncCursor("invalid"), 0);
  assert.equal(parseZapSyncCursor("123"), 123);

  const ownerScope = {
    ownerPubkey: "owner",
    recipientPubkey: "recipient-a",
    relayUrl: "wss://relay.example/",
  };
  assert.match(
    zapSyncCursorStorageKey(ownerScope),
    /^buzz-wallet-zap-sync\.v3:/,
  );
  assert.notEqual(
    zapSyncCursorStorageKey(ownerScope),
    zapSyncCursorStorageKey({
      ...ownerScope,
      recipientPubkey: "recipient-b",
    }),
  );
  assert.notEqual(
    zapSyncCursorStorageKey(ownerScope),
    zapSyncCursorStorageKey({
      ...ownerScope,
      relayUrl: "wss://other.example",
    }),
  );
});

import assert from "node:assert/strict";
import test from "node:test";

import {
  catchUpHostedAgentOwnershipRequests,
  hostedAgentDmChannelKey,
  hostedAgentOwnershipCatchupFilter,
  isHostedAgentOwnershipRequest,
  matchesHostedAgentPurchase,
  ownershipClaimLeaseId,
  ownershipClaimRequestReference,
} from "./hostedAgentOwnership.ts";

const buyer = "11".repeat(32);
const factory = "22".repeat(32);
const plan = "33".repeat(32);
const zapId = "44".repeat(32);
const lease = "8a9fc8ee-34a2-413f-a41a-246420a556ac";
const base = {
  content: "",
  created_at: 1,
  sig: "55".repeat(64),
};
const request = {
  ...base,
  id: "66".repeat(32),
  pubkey: factory,
  kind: 40002,
  tags: [
    ["d", lease],
    ["h", "9bffe474-41f0-412a-97fc-a086a86ece32"],
    ["p", buyer],
    ["agent", "77".repeat(32)],
    ["name", "Silly Elephant"],
    ["e", plan, "", "plan"],
    ["e", zapId, "", "zap"],
  ],
};
const zap = {
  ...base,
  id: zapId,
  pubkey: buyer,
  kind: 9736,
  tags: [
    ["p", factory],
    ["e", plan],
  ],
};

test("matches the ownership request to the buyer zap", () => {
  assert.equal(ownershipClaimRequestReference(request, "zap"), zapId);
  assert.equal(ownershipClaimLeaseId(request), lease);
  assert.equal(matchesHostedAgentPurchase(request, zap, buyer), true);
});

test("rejects a request without its matching buyer zap", () => {
  assert.equal(
    matchesHostedAgentPurchase(request, { ...zap, pubkey: factory }, buyer),
    false,
  );
  assert.equal(
    matchesHostedAgentPurchase(request, { ...zap, tags: [["e", plan]] }, buyer),
    false,
  );
});

test("recognizes an addressed factory ownership request", () => {
  assert.equal(isHostedAgentOwnershipRequest(request, buyer), true);
  assert.equal(
    isHostedAgentOwnershipRequest(
      { ...request, tags: request.tags.filter((tag) => tag[3] !== "zap") },
      buyer,
    ),
    false,
  );
  assert.equal(isHostedAgentOwnershipRequest(request, factory), false);
});

test("catches up stored ownership requests with one addressed query", async () => {
  const ordinaryMessage = {
    ...request,
    id: "88".repeat(32),
    tags: [
      ["h", "9bffe474-41f0-412a-97fc-a086a86ece32"],
      ["p", buyer],
    ],
  };
  const filters = [];
  const accepted = [];

  await catchUpHostedAgentOwnershipRequests({
    buyerPubkey: buyer,
    fetchEvents: async (filter) => {
      filters.push(filter);
      return [ordinaryMessage, request];
    },
    onRequest: async (event) => accepted.push(event.id),
  });

  assert.deepEqual(filters, [hostedAgentOwnershipCatchupFilter(buyer)]);
  assert.deepEqual(filters[0], {
    kinds: [40002],
    "#p": [buyer],
    limit: 1_000,
  });
  assert.deepEqual(accepted, [request.id]);
});

test("stabilizes the hosted-agent DM subscription key", () => {
  const channels = [
    { id: "b", channelType: "dm", isMember: true },
    { id: "ignored", channelType: "stream", isMember: true },
    { id: "a", channelType: "dm", isMember: true },
    { id: "left", channelType: "dm", isMember: false },
  ];

  assert.equal(hostedAgentDmChannelKey(channels), "a,b");
  assert.equal(hostedAgentDmChannelKey([...channels].reverse()), "a,b");
});

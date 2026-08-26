import assert from "node:assert/strict";
import test from "node:test";

import {
  hostedAgentPlanName,
  walletTransactionPresentation,
} from "./walletTransactionPresentation.ts";

function transaction(overrides = {}) {
  return {
    id: "payment",
    direction: "outbound",
    status: "completed",
    statusMessage: "Payment completed",
    amount: 21,
    fees: 0,
    note: "Buzz profile payment intent-a",
    payerNote: "nostr:nipB1:intent-a",
    offerId: null,
    paymentHash: null,
    createdAtMs: 123_000,
    finalizedAtMs: 124_000,
    ...overrides,
  };
}

function zap(overrides = {}) {
  return {
    amount: 21,
    channelId: null,
    comment: "",
    createdAt: 123,
    eventId: "zap-a",
    intentEventId: "intent-a",
    leaseId: null,
    paymentHash: null,
    payerPubkey: "payer",
    recipientName: "",
    recipientPubkey: "recipient",
    targetEventId: null,
    targetEventKind: null,
    ...overrides,
  };
}

function context(zapItem = zap(), overrides = {}) {
  return {
    channelNames: new Map([["channel-a", "general"]]),
    ownedAgentPubkeys: new Set(),
    ownerPubkey: "recipient",
    targetEvents: new Map(),
    userNames: new Map([
      ["payer", "alice"],
      ["recipient", "bob"],
    ]),
    zapsByIntent: new Map([[zapItem.intentEventId, zapItem]]),
    zapsByPaymentHash: new Map(
      zapItem.paymentHash ? [[zapItem.paymentHash, zapItem]] : [],
    ),
    ...overrides,
  };
}

test("formats sent and received profile zaps", () => {
  assert.deepEqual(walletTransactionPresentation(transaction(), context()), {
    title: "Zap sent",
    description: "Zap sent to @bob",
  });
  assert.deepEqual(
    walletTransactionPresentation(
      transaction({ direction: "inbound" }),
      context(),
    ),
    {
      title: "Zap received",
      description: "Zap received from @alice",
    },
  );
});

test("formats message zaps with the user and channel", () => {
  const messageZap = zap({
    channelId: "channel-a",
    targetEventId: "message-a",
  });
  assert.equal(
    walletTransactionPresentation(transaction(), context(messageZap))
      .description,
    "Zap sent to @bob's message in #general",
  );
  assert.equal(
    walletTransactionPresentation(
      transaction({ direction: "inbound" }),
      context(messageZap),
    ).description,
    "Zap received from @alice on message in #general",
  );
});

test("formats hosted-agent lease zaps from the target plan", () => {
  const leaseZap = zap({
    channelId: "channel-a",
    leaseId: "lease-a",
    targetEventId: "plan-a",
  });
  const planEvent = {
    id: "plan-a",
    pubkey: "recipient",
    created_at: 123,
    kind: 39007,
    tags: [
      ["d", "hosted-agent"],
      ["agent_host_plan", JSON.stringify({ name: "researcher" })],
    ],
    content: "",
    sig: "signature",
  };
  const leaseContext = context(leaseZap, {
    targetEvents: new Map([["plan-a", planEvent]]),
  });
  assert.equal(hostedAgentPlanName(planEvent), "researcher");
  assert.equal(
    walletTransactionPresentation(transaction(), leaseContext).description,
    "Zap sent to @bob to lease agent @researcher",
  );
  assert.equal(
    walletTransactionPresentation(
      transaction({ direction: "inbound" }),
      leaseContext,
    ).description,
    "Zap received from @alice to lease agent @researcher",
  );
  assert.equal(
    walletTransactionPresentation(transaction(), context(leaseZap)).description,
    "Zap sent to @bob to lease an agent",
  );
});

test("hides an internal zap ID until its proof is cached", () => {
  assert.deepEqual(
    walletTransactionPresentation(
      transaction(),
      context(zap(), {
        zapsByIntent: new Map(),
      }),
    ),
    {
      title: "Payment sent",
      description: "Payment completed",
    },
  );
});

test("matches an inbound agent-zap record by payment hash", () => {
  const paymentHash = "ab".repeat(32);
  const agentZap = zap({ paymentHash });
  assert.deepEqual(
    walletTransactionPresentation(
      transaction({
        direction: "inbound",
        note: null,
        payerNote: null,
        paymentHash,
      }),
      context(agentZap, {
        ownedAgentPubkeys: new Set(["recipient"]),
        ownerPubkey: "owner",
      }),
    ),
    {
      title: "Zap received",
      description: "Zap received by @bob from @alice",
    },
  );
});

test("names the managed agent in received message zaps", () => {
  const agentZap = zap({
    channelId: "channel-a",
    recipientName: "Bob Agent",
    targetEventId: "message-a",
  });
  assert.equal(
    walletTransactionPresentation(
      transaction({ direction: "inbound" }),
      context(agentZap, { ownerPubkey: "owner" }),
    ).description,
    "Zap received by @bob from @alice on message in #general",
  );
});

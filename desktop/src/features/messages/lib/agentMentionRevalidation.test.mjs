import assert from "node:assert/strict";
import test from "node:test";

import { revalidateAgentMentionPubkeys } from "./agentMentionRevalidation.ts";

const CURRENT = "a".repeat(64);
const AGENT = "b".repeat(64);
const HUMAN = "c".repeat(64);
const OTHER_OWNER = "d".repeat(64);
const LOCAL_AGENT = "e".repeat(64);

function options(
  refetchOwnerProfiles = async (pubkeys) => ({
    profiles: pubkeys.includes(AGENT)
      ? { [AGENT]: { ownerPubkey: CURRENT } }
      : {},
    missing: pubkeys.filter((pubkey) => pubkey !== AGENT),
  }),
) {
  return {
    pubkeys: [HUMAN, AGENT],
    agentPubkeys: new Set([AGENT]),
    currentPubkey: CURRENT,
    eligibilityScope: { type: "channel", channelId: "general" },
    sharedChannelIds: new Set(["general"]),
    ownerOnly: false,
    ownerPolicyError: null,
    refetchManagedAgents: async () => ({ data: [], error: null }),
    fetchRelayAgents: async () => [
      {
        pubkey: AGENT,
        respondTo: "anyone",
        respondToAllowlist: [],
        channelIds: ["general"],
      },
    ],
    refetchOwnerProfiles,
  };
}

test("relay policy revalidation admits an authorized external agent", async () => {
  assert.deepEqual(await revalidateAgentMentionPubkeys(options()), [
    HUMAN,
    AGENT,
  ]);
});

test("fresh managed evidence survives unrelated relay authorization errors", async () => {
  const result = await revalidateAgentMentionPubkeys({
    ...options(),
    pubkeys: [HUMAN, LOCAL_AGENT],
    agentPubkeys: new Set([LOCAL_AGENT]),
    refetchManagedAgents: async () => ({
      data: [{ pubkey: LOCAL_AGENT }],
      error: null,
    }),
    fetchRelayAgents: async () => {
      throw new Error("relay directory unavailable");
    },
  });

  assert.deepEqual(result, [HUMAN, LOCAL_AGENT]);
});

test("verified owned agents do not depend on relay discovery", async () => {
  const result = await revalidateAgentMentionPubkeys({
    ...options(),
    fetchRelayAgents: async () => {
      throw new Error("relay directory unavailable");
    },
  });

  assert.deepEqual(result, [HUMAN, AGENT]);
});

test("an owned agent survives send-time revalidation", async () => {
  const result = await revalidateAgentMentionPubkeys({
    ...options(async () => ({
      profiles: {
        [AGENT]: {
          ownerPubkey: CURRENT,
        },
      },
      missing: [],
    })),
    fetchRelayAgents: async () => {
      throw new Error("relay directory unavailable");
    },
  });

  assert.deepEqual(result, [HUMAN, AGENT]);
});

test("an owned directory agent can be added to another channel", async () => {
  const result = await revalidateAgentMentionPubkeys({
    ...options(async () => ({ profiles: {}, missing: [AGENT] })),
    fetchRelayAgents: async () => [
      {
        pubkey: AGENT,
        ownerPubkey: CURRENT,
        respondTo: "anyone",
        respondToAllowlist: [],
        channelIds: ["agent-factory"],
      },
    ],
  });

  assert.deepEqual(result, [HUMAN, AGENT]);
});

test("an OSS build keeps an owned agent before its channel invite", async () => {
  const requested = [];
  const result = await revalidateAgentMentionPubkeys({
    ...options(async (pubkeys) => {
      requested.push(...pubkeys);
      return {
        profiles: { [AGENT]: { ownerPubkey: CURRENT } },
        missing: [],
      };
    }),
    ownerOnly: false,
    fetchRelayAgents: async () => [],
  });

  assert.deepEqual(requested, [AGENT]);
  assert.deepEqual(result, [HUMAN, AGENT]);
});

test("an agent owned by another user fails closed", async () => {
  const result = await revalidateAgentMentionPubkeys({
    ...options(async () => ({
      profiles: {
        [AGENT]: {
          ownerPubkey: OTHER_OWNER,
        },
      },
      missing: [],
    })),
    fetchRelayAgents: async () => {
      throw new Error("relay directory unavailable");
    },
  });

  assert.deepEqual(result, [HUMAN]);
});

test("mixed evidence preserves managed and verified owned agents", async () => {
  const result = await revalidateAgentMentionPubkeys({
    ...options(async () => ({
      profiles: { [AGENT]: { ownerPubkey: CURRENT } },
      missing: [LOCAL_AGENT],
    })),
    pubkeys: [HUMAN, LOCAL_AGENT, AGENT],
    agentPubkeys: new Set([LOCAL_AGENT, AGENT]),
    refetchManagedAgents: async () => ({
      data: [{ pubkey: LOCAL_AGENT }],
      error: null,
    }),
    fetchRelayAgents: async () => {
      throw new Error("relay directory unavailable");
    },
  });

  assert.deepEqual(result, [HUMAN, LOCAL_AGENT, AGENT]);
});

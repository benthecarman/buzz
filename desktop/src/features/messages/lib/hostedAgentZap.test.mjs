import assert from "node:assert/strict";
import test from "node:test";

import {
  hostedAgentPlanMessage,
  hostedAgentZapTarget,
} from "./hostedAgentZap.ts";

function message(tags, id = "message-id", kind = 40002) {
  return { id, kind, tags };
}

function planMessage(tags, id = "message-id") {
  return message([["d", "hosted-agent"], ...tags], id, 39007);
}

test("plan selects an hourly creation zap", () => {
  const plan = {
    version: 1,
    name: "Codex workspace",
    hourly_price_sats: 500,
    retention_days: 30,
    harness_profile: "codex",
    model: "gpt-test",
    system_prompt: "You are a hosted agent.",
  };
  assert.deepEqual(
    hostedAgentZapTarget(
      planMessage(
        [
          ["h", "source-channel"],
          ["agent_host_plan", JSON.stringify(plan)],
        ],
        "plan-id",
      ),
      "source-channel",
    ),
    {
      amount: 500,
      channelId: "source-channel",
      leaseId: null,
      targetEventId: "plan-id",
    },
  );
  assert.deepEqual(
    hostedAgentPlanMessage(
      planMessage(
        [
          ["h", "source-channel"],
          ["agent_host_plan", JSON.stringify(plan)],
        ],
        "plan-id",
      ),
      "source-channel",
    ),
    {
      channelId: "source-channel",
      harnessProfile: "codex",
      hourlyPriceSats: 500,
      model: "gpt-test",
      name: "Codex workspace",
      retentionDays: 30,
      systemPrompt: "You are a hosted agent.",
      targetEventId: "plan-id",
    },
  );
  assert.equal(
    hostedAgentPlanMessage(
      message(
        [
          ["d", "hosted-agent"],
          ["h", "source-channel"],
          ["agent_host_plan", JSON.stringify(plan)],
        ],
        "regular-message-id",
      ),
      "source-channel",
    ),
    null,
  );
  assert.equal(
    hostedAgentPlanMessage(
      message(
        [
          ["d", "wrong-plan"],
          ["h", "source-channel"],
          ["agent_host_plan", JSON.stringify(plan)],
        ],
        "wrong-address-id",
        39007,
      ),
      "source-channel",
    ),
    null,
  );
});

test("plan reply renders and buys the stored plan", () => {
  const plan = {
    version: 1,
    name: "Codex workspace",
    hourly_price_sats: 500,
    retention_days: 30,
    harness_profile: "codex",
    model: "gpt-test",
    system_prompt: "You are a hosted agent.",
  };
  const reply = message(
    [
      ["h", "request-channel"],
      ["agent_host_plan", JSON.stringify(plan)],
      ["agent_host_plan_ref", "stored-plan-id", "source-channel"],
    ],
    "reply-id",
    9,
  );

  assert.deepEqual(hostedAgentPlanMessage(reply, "request-channel"), {
    channelId: "source-channel",
    harnessProfile: "codex",
    hourlyPriceSats: 500,
    model: "gpt-test",
    name: "Codex workspace",
    retentionDays: 30,
    systemPrompt: "You are a hosted agent.",
    targetEventId: "stored-plan-id",
  });
  assert.deepEqual(hostedAgentZapTarget(reply, "request-channel"), {
    amount: 500,
    channelId: "source-channel",
    leaseId: null,
    targetEventId: "stored-plan-id",
  });
});

test("receipt selects the original plan and lease", () => {
  const receipt = {
    hourly_price_sats: 500,
    channel_id: "source-channel",
    lease_id: "lease-id",
    plan_event_id: "plan-id",
  };
  assert.deepEqual(
    hostedAgentZapTarget(
      message([["hosted_agent_receipt", JSON.stringify(receipt)]]),
    ),
    {
      amount: 500,
      channelId: "source-channel",
      leaseId: "lease-id",
      targetEventId: "plan-id",
    },
  );
});

test("invalid or duplicate plan tags do not start a payment", () => {
  assert.equal(
    hostedAgentZapTarget(
      planMessage([
        ["h", "source-channel"],
        ["agent_host_plan", "not-json"],
      ]),
      "source-channel",
    ),
    null,
  );
  const plan = JSON.stringify({
    version: 1,
    name: "Codex workspace",
    hourly_price_sats: 500,
    retention_days: 30,
    harness_profile: "codex",
  });
  assert.equal(
    hostedAgentZapTarget(
      planMessage([
        ["h", "source-channel"],
        ["agent_host_plan", plan],
        ["agent_host_plan", plan],
      ]),
      "source-channel",
    ),
    null,
  );
});

test("plan must match the visible channel and complete contract", () => {
  const plan = JSON.stringify({
    version: 1,
    name: "Codex workspace",
    hourly_price_sats: 500,
    retention_days: 30,
    harness_profile: "codex",
  });
  assert.equal(
    hostedAgentZapTarget(
      planMessage([
        ["h", "other-channel"],
        ["agent_host_plan", plan],
      ]),
      "source-channel",
    ),
    null,
  );
  assert.equal(
    hostedAgentPlanMessage(
      planMessage([
        ["h", "source-channel"],
        [
          "agent_host_plan",
          JSON.stringify({
            version: 0,
            name: "Legacy plan",
            hourly_price_sats: 500,
            retention_days: 30,
            harness_profile: "codex",
          }),
        ],
      ]),
      "source-channel",
    ),
    null,
  );
  assert.equal(
    hostedAgentZapTarget(
      planMessage([
        ["h", "source-channel"],
        [
          "agent_host_plan",
          JSON.stringify({ version: 1, hourly_price_sats: 500 }),
        ],
      ]),
      "source-channel",
    ),
    null,
  );
  assert.equal(
    hostedAgentPlanMessage(
      planMessage([
        ["h", "source-channel"],
        [
          "agent_host_plan",
          JSON.stringify({
            version: 1,
            name: "Incomplete plan",
            hourly_price_sats: 500,
            retention_days: 30,
            harness_profile: "codex",
          }),
        ],
      ]),
      "source-channel",
    ),
    null,
  );
});

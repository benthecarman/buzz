import assert from "node:assert/strict";
import test from "node:test";

import {
  hostedAgentPlanMessage,
  hostedAgentZapTarget,
} from "./hostedAgentZap.ts";

function message(tags, id = "message-id") {
  return { id, tags };
}

test("plan selects an hourly creation zap", () => {
  const plan = {
    version: 1,
    name: "Codex workspace",
    hourly_price_sats: 500,
    retention_days: 30,
    harness_profile: "codex",
  };
  assert.deepEqual(
    hostedAgentZapTarget(
      message(
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
      message(
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
      name: "Codex workspace",
      retentionDays: 30,
      targetEventId: "plan-id",
    },
  );
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
      message([
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
      message([
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
      message([
        ["h", "other-channel"],
        ["agent_host_plan", plan],
      ]),
      "source-channel",
    ),
    null,
  );
  assert.equal(
    hostedAgentZapTarget(
      message([
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
});

import assert from "node:assert/strict";
import test from "node:test";

import {
  agentRuntimePackChargeSats,
  agentRuntimePackRequired,
} from "./runtimePayments.ts";

test("retained runtime avoids a zap only when it covers the full cap", () => {
  assert.equal(agentRuntimePackRequired(30 * 60_000, 30), false);
  assert.equal(agentRuntimePackRequired(30 * 60_000 - 1, 30), true);
});

test("an insufficient balance buys a full pack matching the selected cap", () => {
  assert.equal(agentRuntimePackChargeSats(59 * 60_000, 60, 20), 1_200);
  assert.equal(agentRuntimePackChargeSats(60 * 60_000, 60, 20), 0);
  assert.equal(agentRuntimePackChargeSats(0, 15, 7), 105);
});

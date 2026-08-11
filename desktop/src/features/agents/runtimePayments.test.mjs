import assert from "node:assert/strict";
import test from "node:test";

import { activeAccessZap } from "./runtimePayments.ts";

test("a settled zap stays active through its final second", () => {
  const status = {
    accessZap: { zapEventId: "a".repeat(64), createdAt: 100, validUntil: 400 },
    pricing: null,
  };
  assert.ok(activeAccessZap(status, 400));
  assert.equal(activeAccessZap(status, 401), null);
});

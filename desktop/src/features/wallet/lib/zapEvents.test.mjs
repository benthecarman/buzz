import assert from "node:assert/strict";
import test from "node:test";

import { zapSubscriptionFilter } from "./zapEvents.ts";

test("zap subscription covers the owner and agents once", () => {
  assert.deepEqual(zapSubscriptionFilter(["AA", "bb", "aa"], 123), {
    kinds: [9736],
    "#p": ["aa", "bb"],
    limit: 50,
    since: 123,
  });
});

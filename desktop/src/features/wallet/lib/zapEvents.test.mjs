import assert from "node:assert/strict";
import test from "node:test";

import {
  zapLiveSubscriptionFilters,
  zapSubscriptionFilter,
} from "./zapEvents.ts";

test("zap subscription covers the owner and agents once", () => {
  assert.deepEqual(zapSubscriptionFilter(["AA", "bb", "aa"], 123), {
    kinds: [9736],
    "#p": ["aa", "bb"],
    limit: 50,
    since: 123,
  });
});

test("zap live subscriptions use one exact channel scope per filter", () => {
  assert.deepEqual(
    zapLiveSubscriptionFilters(["owner"], 123, [
      "channel-b",
      "channel-a",
      "channel-b",
    ]),
    [
      { kinds: [9736], "#p": ["owner"], limit: 50, since: 123 },
      {
        kinds: [9736],
        "#p": ["owner"],
        "#h": ["channel-a"],
        limit: 50,
        since: 123,
      },
      {
        kinds: [9736],
        "#p": ["owner"],
        "#h": ["channel-b"],
        limit: 50,
        since: 123,
      },
    ],
  );
});

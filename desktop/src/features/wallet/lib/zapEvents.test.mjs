import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";

import {
  zapAuthorSubscriptionFilter,
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

test("zap author subscription covers sent proofs once", () => {
  assert.deepEqual(zapAuthorSubscriptionFilter(["AA", "bb", "aa"], 123), {
    kinds: [9736],
    authors: ["aa", "bb"],
    limit: 50,
    since: 123,
  });
});

test("zap live subscriptions use one exact channel scope per filter", () => {
  assert.deepEqual(
    zapLiveSubscriptionFilters(["owner"], ["owner", "agent"], 123, [
      "channel-b",
      "channel-a",
      "channel-b",
    ]),
    [
      { kinds: [9736], "#p": ["owner"], limit: 50, since: 123 },
      {
        kinds: [9736],
        authors: ["owner", "agent"],
        limit: 50,
        since: 123,
      },
      {
        kinds: [9736],
        "#p": ["owner"],
        "#h": ["channel-a"],
        limit: 50,
        since: 123,
      },
      {
        kinds: [9736],
        authors: ["owner", "agent"],
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
      {
        kinds: [9736],
        authors: ["owner", "agent"],
        "#h": ["channel-b"],
        limit: 50,
        since: 123,
      },
    ],
  );
});

test("received zap UI never reads wallet state", () => {
  const sources = [
    "src/features/wallet/useZapNotifications.ts",
    "src/features/wallet/lib/useVerifiedZapEvents.ts",
    "src/features/wallet/lib/zapNotificationSync.ts",
    "src/features/wallet/lib/zapHistory.ts",
    "src/features/messages/ui/MessageReactions.tsx",
  ].map((path) => [path, readFileSync(path, "utf8")]);
  const forbidden = [
    "getWalletStatus",
    "listWalletTransactions",
    "listPlaceholderMessageZaps",
    "usePlaceholderMessageZaps",
  ];

  for (const [path, source] of sources) {
    for (const name of forbidden) {
      assert.equal(
        source.includes(name),
        false,
        `${path} must not use ${name}`,
      );
    }
  }
});

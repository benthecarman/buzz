import assert from "node:assert/strict";
import test from "node:test";

import { placeholderMessageZapsQueryKey } from "./placeholderMessageZaps.ts";

test("placeholder message zap query key is wallet scoped", () => {
  assert.deepEqual(placeholderMessageZapsQueryKey, [
    "wallet",
    "placeholder-message-zaps",
  ]);
});

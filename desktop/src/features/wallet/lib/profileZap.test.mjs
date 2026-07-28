import assert from "node:assert/strict";
import test from "node:test";

import { parseWholeBitcoinAmount } from "./profileZap.ts";

test("profile payments accept only positive whole Bitcoin units", () => {
  assert.equal(parseWholeBitcoinAmount("21000"), 21_000);
  assert.equal(parseWholeBitcoinAmount("0"), null);
  assert.equal(parseWholeBitcoinAmount("-1"), null);
  assert.equal(parseWholeBitcoinAmount("1.5"), null);
  assert.equal(parseWholeBitcoinAmount("1e3"), null);
  assert.equal(parseWholeBitcoinAmount("21,000"), null);
  assert.equal(parseWholeBitcoinAmount("9007199254740992"), null);
});

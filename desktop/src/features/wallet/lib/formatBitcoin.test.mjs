import assert from "node:assert/strict";
import test from "node:test";

import { formatBitcoin } from "./formatBitcoin.ts";

test("formats whole bitcoin quantities with the bitcoin symbol", () => {
  assert.equal(formatBitcoin(21_000), "₿ 21,000");
  assert.equal(formatBitcoin(0), "₿ 0");
  assert.equal(formatBitcoin(null), "₿ —");
  assert.equal(formatBitcoin(21_000).includes("sats"), false);
});

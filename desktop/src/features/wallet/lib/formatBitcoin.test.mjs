import assert from "node:assert/strict";
import test from "node:test";

import {
  formatBitcoin,
  formatSatsAsUsd,
  setBitcoinUsdRate,
} from "./formatBitcoin.ts";

test("formats whole bitcoin quantities with the bitcoin symbol", () => {
  setBitcoinUsdRate(null);
  assert.equal(formatBitcoin(21_000), "₿ 21,000");
  assert.equal(formatBitcoin(0), "₿ 0");
  assert.equal(formatBitcoin(null), "₿ —");
  assert.equal(formatBitcoin(21_000).includes("sats"), false);
});

test("formats USD separately from the bitcoin amount", () => {
  setBitcoinUsdRate(80_000);
  assert.equal(formatSatsAsUsd(250), "$0.20");
  assert.equal(formatBitcoin(250), "₿ 250");
  setBitcoinUsdRate(null);
});

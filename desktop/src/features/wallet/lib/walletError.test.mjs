import assert from "node:assert/strict";
import test from "node:test";

import { walletCommandError } from "./walletError.ts";

test("preserves the structured code wrapped by a Tauri invoke error", () => {
  const error = new Error("runtime quote has expired");
  error.payload = {
    code: "runtime_quote_expired",
    message: "runtime quote has expired",
  };

  assert.deepEqual(walletCommandError(error), {
    code: "runtime_quote_expired",
    message: "runtime quote has expired",
  });
});

import assert from "node:assert/strict";
import test from "node:test";

import {
  createWalletReceiveRequest,
  disableWallet,
  enableWallet,
  getRecipientWalletOffer,
  refreshWalletOffer,
  sendProfileZap,
} from "./api.ts";

const RECIPIENT_PUBKEY =
  "bb22a5299220cad76ffd46190ccbeede8ab5dc260faa28b6e5a2cb31b9aff260";
const IDEMPOTENCY_KEY = "d2c7ac5e-8ebf-4b85-a5fc-3a693cdadf71";

test("wallet commands resolve the active relay backend-side", async () => {
  const previousWindow = globalThis.window;
  const calls = [];

  globalThis.window = {
    __TAURI_INTERNALS__: {
      invoke(command, args) {
        calls.push({ command, args });
        return Promise.resolve({});
      },
    },
  };

  try {
    await enableWallet();
    await createWalletReceiveRequest();
    await refreshWalletOffer();
    await disableWallet();
    await getRecipientWalletOffer(RECIPIENT_PUBKEY);
    await sendProfileZap({
      recipientPubkey: RECIPIENT_PUBKEY,
      amount: 21,
      comment: null,
      idempotencyKey: IDEMPOTENCY_KEY,
    });
  } finally {
    globalThis.window = previousWindow;
  }

  // No command takes a caller-supplied relay list: the backend resolves the
  // active workspace relay itself, like every other command in the codebase.
  assert.deepEqual(calls, [
    { command: "wallet_enable", args: {} },
    { command: "wallet_create_receive_request", args: {} },
    { command: "wallet_refresh_offer", args: {} },
    { command: "wallet_disable", args: {} },
    {
      command: "wallet_get_recipient_offer",
      args: { recipientPubkey: RECIPIENT_PUBKEY },
    },
    {
      command: "wallet_send_profile_zap",
      args: {
        request: {
          recipientPubkey: RECIPIENT_PUBKEY,
          amount: 21,
          comment: null,
          idempotencyKey: IDEMPOTENCY_KEY,
        },
      },
    },
  ]);
});

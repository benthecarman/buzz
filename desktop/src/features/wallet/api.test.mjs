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
const TARGET_EVENT_ID = "ab".repeat(32);

test("offer publication commands include every configured community relay", async () => {
  const previousLocalStorage = globalThis.localStorage;
  const previousWindow = globalThis.window;
  const calls = [];
  const communities = [
    {
      id: "community-a",
      name: "A",
      relayUrl: "wss://relay-a.example",
      addedAt: "2026-01-01T00:00:00.000Z",
    },
    {
      id: "community-b",
      name: "B",
      relayUrl: "wss://relay-b.example",
      addedAt: "2026-01-02T00:00:00.000Z",
    },
  ];

  globalThis.localStorage = {
    getItem(key) {
      if (key === "buzz-communities") return JSON.stringify(communities);
      return null;
    },
    setItem() {},
    removeItem() {},
  };
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
      targetEventId: TARGET_EVENT_ID,
      targetEventKind: 40_002,
    });
  } finally {
    globalThis.localStorage = previousLocalStorage;
    globalThis.window = previousWindow;
  }

  // Our own offer announcement (and its withdrawal) must reach every
  // community relay. Lookups take no caller-supplied relay list: a recipient
  // has to be in our community, so the backend queries the active workspace
  // relay it resolves itself, like every other command in the codebase.
  const relayUrls = ["wss://relay-a.example", "wss://relay-b.example"];
  assert.deepEqual(calls, [
    { command: "wallet_enable", args: { relayUrls } },
    { command: "wallet_create_receive_request", args: {} },
    { command: "wallet_refresh_offer", args: { relayUrls } },
    { command: "wallet_disable", args: { relayUrls } },
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
          targetEventId: TARGET_EVENT_ID,
          targetEventKind: 40_002,
        },
      },
    },
  ]);
});

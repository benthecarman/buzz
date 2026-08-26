import { expect, test } from "@playwright/test";

import { installMockBridge, TEST_IDENTITIES } from "../helpers/bridge";
import { FEATURE_OVERRIDES_STORAGE_KEY } from "../helpers/features";
import { openSettings } from "../helpers/settings";

const MOCK_OWNER_PUBKEY = "deadbeef".repeat(8);
const MOCK_RELAY_URL = (
  process.env.BUZZ_E2E_RELAY_URL ?? "http://localhost:3000"
).replace(/^http/, "ws");
const AGENT_ZAP_PAYMENT_HASH = "b".repeat(64);
const PROFILE_ZAP_INTENT_ID = "a".repeat(64);

test.beforeEach(async ({ page }) => {
  await page.route(
    "https://api.coinbase.com/v2/prices/BTC-USD/spot",
    async (route) => {
      await route.fulfill({
        contentType: "application/json",
        body: JSON.stringify({
          data: { amount: "80000", currency: "USD" },
        }),
      });
    },
  );
  await page.addInitScript(
    ({ storageKey }) => {
      window.localStorage.setItem(
        storageKey,
        JSON.stringify({ bitcoin: true }),
      );
    },
    { storageKey: FEATURE_OVERRIDES_STORAGE_KEY },
  );
  await page.addInitScript(
    ({
      intentEventId,
      ownerPubkey,
      paymentHash,
      recipientPubkey,
      relayUrl,
    }) => {
      const key = `buzz-wallet-zap-history.v1:${ownerPubkey}:${encodeURIComponent(relayUrl)}`;
      window.localStorage.setItem(
        key,
        JSON.stringify([
          {
            amount: 50,
            channelId: null,
            comment: "",
            createdAt: 1_700_000_002,
            eventId: "profile-zap-event",
            intentEventId,
            leaseId: null,
            paymentHash,
            payerPubkey: ownerPubkey,
            recipientName: "",
            recipientPubkey,
            targetEventId: null,
            targetEventKind: null,
          },
        ]),
      );
    },
    {
      intentEventId: PROFILE_ZAP_INTENT_ID,
      ownerPubkey: MOCK_OWNER_PUBKEY,
      paymentHash: AGENT_ZAP_PAYMENT_HASH,
      recipientPubkey: TEST_IDENTITIES.alice.pubkey,
      relayUrl: MOCK_RELAY_URL,
    },
  );
  await installMockBridge(page, {
    relayAgents: [
      {
        pubkey: TEST_IDENTITIES.alice.pubkey,
        name: "Alice Agent",
        ownerPubkey: MOCK_OWNER_PUBKEY,
      },
    ],
    walletBalance: 21_000,
    walletSpendableBalance: 20_000,
    walletTransactions: [
      {
        id: "received-transaction",
        direction: "inbound",
        status: "completed",
        statusMessage: "Payment completed",
        amount: 5_000,
        fees: 10,
        note: "Test payment",
        payerNote: null,
        offerId: null,
        paymentHash: null,
        createdAtMs: 1_700_000_000_000,
        finalizedAtMs: 1_700_000_001_000,
      },
      {
        id: "sent-profile-zap",
        direction: "outbound",
        status: "completed",
        statusMessage: "Payment completed",
        amount: 50,
        fees: 1,
        note: `Buzz profile payment ${PROFILE_ZAP_INTENT_ID}`,
        payerNote: `nostr:nipB1:${PROFILE_ZAP_INTENT_ID}`,
        offerId: "long-provider-offer-identifier",
        paymentHash: AGENT_ZAP_PAYMENT_HASH,
        createdAtMs: 1_700_000_002_000,
        finalizedAtMs: 1_700_000_003_000,
      },
      {
        id: "received-agent-zap",
        direction: "inbound",
        status: "completed",
        statusMessage: "Payment completed",
        amount: 49,
        fees: 0,
        note: null,
        payerNote: null,
        offerId: "agent-offer-identifier",
        paymentHash: AGENT_ZAP_PAYMENT_HASH,
        createdAtMs: 1_700_000_001_500,
        finalizedAtMs: 1_700_000_002_500,
      },
    ],
  });
});

test("profile menu shows the spendable balance and opens wallet settings", async ({
  page,
}) => {
  await page.goto("/");
  await page.getByTestId("sidebar-profile-avatar-button").click();

  const balance = page.getByTestId("profile-popover-wallet-balance");
  await expect(balance).toBeVisible();
  await expect(balance).toHaveText("20,000");
  await expect(balance).not.toContainText("$");
  await expect(balance).toHaveAttribute(
    "aria-label",
    "Open wallet settings. Spendable balance ₿ 20,000",
  );

  await balance.click();

  await expect(page).toHaveURL(/\/settings\?section=wallet$/);
  await expect(page.getByTestId("settings-wallet")).toBeVisible();
});

test("profile menu hides the balance when the Bitcoin experiment is disabled", async ({
  page,
}) => {
  await page.goto("/");
  await openSettings(page);
  await page.evaluate((storageKey) => {
    window.localStorage.setItem(storageKey, JSON.stringify({ bitcoin: false }));
    window.dispatchEvent(new StorageEvent("storage", { key: storageKey }));
  }, FEATURE_OVERRIDES_STORAGE_KEY);
  await expect(page.getByTestId("settings-nav-wallet")).toHaveCount(0);

  await page.getByTestId("settings-back-to-app").click();
  await page.getByTestId("sidebar-profile-avatar-button").click();

  await expect(page.getByTestId("profile-popover-wallet-balance")).toHaveCount(
    0,
  );
});

test("wallet balance exposes funding and transfer actions", async ({
  page,
}) => {
  await page.goto("/");
  await openSettings(page, "wallet");

  await expect(page.getByTestId("settings-wallet")).toBeVisible();
  await expect(page.getByTestId("wallet-spendable-balance")).toHaveText(
    "₿ 20,000",
  );
  await expect(page.getByTestId("wallet-spendable-balance-usd")).toHaveText(
    "$16.00",
  );
  const transaction = page.getByTestId(
    "wallet-transaction-received-transaction",
  );
  await expect(transaction).toContainText("+₿ 5,000");
  await expect(page.getByText("Zap history", { exact: true })).toHaveCount(0);
  await expect(
    transaction.getByTestId("wallet-transaction-amount-usd"),
  ).toHaveText("+$4.00");
  await expect(page.getByText("Reserved funds")).toHaveCount(0);
  const sentZap = page.getByTestId("wallet-transaction-sent-profile-zap");
  await expect(sentZap).toContainText("Zap sent");
  await expect(sentZap).toContainText("Zap sent to @alice");
  await expect(sentZap).not.toContainText(PROFILE_ZAP_INTENT_ID);
  await expect(sentZap).not.toContainText("long-provider-offer-identifier");
  const receivedAgentZap = page.getByTestId(
    "wallet-transaction-received-agent-zap",
  );
  await expect(receivedAgentZap).toContainText("Zap received");
  await expect(receivedAgentZap).toContainText("Zap received by @alice from @");
  await expect(receivedAgentZap).not.toContainText("Payment received");
  await expect(receivedAgentZap).not.toContainText("agent-offer-identifier");
  await expect(page.getByTestId("wallet-receive-qr")).toHaveCount(0);

  await page.getByRole("button", { name: "Fund wallet" }).click();

  await expect(page.getByTestId("wallet-receive-qr")).toBeVisible();
  await expect(
    page.getByText("Transfer bitcoin from an external Lightning wallet"),
  ).toBeVisible();
  await expect(page.getByTestId("wallet-receive-qr")).toHaveAttribute(
    "width",
    "346",
  );
  await expect(page.getByRole("button", { name: "Copy" })).toBeVisible();

  await page.getByRole("button", { name: "Transfer out" }).click();
  await expect(page.getByTestId("wallet-receive-qr")).toHaveCount(0);
  await expect(
    page.getByRole("textbox", { name: "Destination" }),
  ).toBeVisible();
  await expect(
    page.getByRole("textbox", { name: "Amount to send" }),
  ).toBeVisible();
  await expect(page.getByRole("textbox", { name: "Note" })).toBeVisible();
  await expect(
    page.getByRole("button", { name: "Send payment" }),
  ).toBeVisible();
});

test("wallet settings changes only the default for new agents", async ({
  page,
}) => {
  const agentPubkey = "cd".repeat(32);
  await page.goto("/");
  await page.evaluate((pubkey) => {
    const testWindow = window as Window & {
      __BUZZ_E2E__?: {
        mock?: {
          walletNwcClients?: unknown[];
        };
      };
    };
    if (!testWindow.__BUZZ_E2E__?.mock) {
      throw new Error("mock bridge config is unavailable");
    }
    testWindow.__BUZZ_E2E__.mock.walletNwcClients = [
      {
        agentPubkey: pubkey,
        agentName: "Steady Finch",
        mode: "manual",
        budgetAmount: null,
        budgetPeriod: null,
        spentAmount: 0,
        remainingAmount: null,
        periodEndsAtMs: null,
      },
    ];
  }, agentPubkey);
  await openSettings(page, "wallet");

  const card = page.getByTestId("wallet-agent-spending");
  await expect(card).toContainText("Default agent budget");
  await expect(card).toContainText("Default for new agents");
  await expect(card).toContainText(
    "Existing agents keep their current budgets",
  );
  await expect(card).not.toContainText("Steady Finch");

  await page.getByTestId("wallet-default-agent-mode-budget").click();
  await page
    .getByRole("textbox", { name: "Default budget for new agents" })
    .fill("300");
  await page.getByTestId("wallet-default-agent-period-month").click();
  await page.getByRole("button", { name: "Save" }).click();

  await expect(
    page
      .locator("[data-sonner-toast]")
      .filter({ hasText: "Default agent budget was updated" }),
  ).toBeVisible();
  const commands = await page.evaluate(
    () =>
      (
        window as Window & {
          __BUZZ_E2E_COMMANDS__?: string[];
        }
      ).__BUZZ_E2E_COMMANDS__ ?? [],
  );
  expect(
    commands.filter((command) => command === "wallet_set_default_nwc_policy"),
  ).toHaveLength(1);
  expect(
    commands.filter((command) => command === "wallet_list_nwc_clients"),
  ).toHaveLength(0);
  expect(
    commands.filter((command) => command === "wallet_set_nwc_policy"),
  ).toHaveLength(0);
});

test("transfer out is disabled when the wallet is empty", async ({ page }) => {
  await page.goto("/");
  await page.evaluate(() => {
    const testWindow = window as Window & {
      __BUZZ_E2E__?: {
        mock?: {
          walletBalance?: number;
          walletSpendableBalance?: number;
        };
      };
    };
    if (!testWindow.__BUZZ_E2E__?.mock) {
      throw new Error("mock bridge config is unavailable");
    }
    testWindow.__BUZZ_E2E__.mock.walletBalance = 0;
    testWindow.__BUZZ_E2E__.mock.walletSpendableBalance = 0;
  });
  await openSettings(page, "wallet");

  await expect(
    page.getByRole("button", { name: "Transfer out" }),
  ).toBeDisabled();
});

test("transfer out sends a required amount without a review step", async ({
  page,
}) => {
  await page.goto("/");
  await openSettings(page, "wallet");
  await page.getByRole("button", { name: "Transfer out" }).click();

  const amount = page.getByRole("textbox", { name: "Amount to send:" });
  const destination = page.getByRole("textbox", { name: "Destination:" });
  await expect(amount).toHaveAttribute("required", "");
  await amount.fill("500");
  await destination.fill("lno1mockoffer");
  await page.getByRole("button", { name: "Send payment" }).click();

  await expect(page.getByText("Confirm payment")).toHaveCount(0);
  await expect(
    page
      .locator("[data-sonner-toast]")
      .filter({ hasText: "Payment completed: ₿ 500" }),
  ).toBeVisible();
  const commands = await page.evaluate(
    () =>
      (
        window as Window & {
          __BUZZ_E2E_COMMANDS__?: string[];
        }
      ).__BUZZ_E2E_COMMANDS__ ?? [],
  );
  expect(
    commands.filter((command) => command === "wallet_analyze_destination"),
  ).toHaveLength(1);
  expect(commands.filter((command) => command === "wallet_send")).toHaveLength(
    1,
  );
});

test("transfer out reconciles the exact persisted request without analysis", async ({
  page,
}) => {
  await page.goto("/");
  await page.evaluate(() => {
    const testWindow = window as Window & {
      __BUZZ_E2E__?: {
        mock?: {
          walletPendingSend?: {
            destination: string;
            amount: number | null;
            message: string | null;
            requestId: string;
          };
        };
      };
    };
    if (!testWindow.__BUZZ_E2E__?.mock) {
      throw new Error("mock bridge config is unavailable");
    }
    testWindow.__BUZZ_E2E__.mock.walletPendingSend = {
      destination: "lnbc1fixedmockinvoice",
      amount: null,
      message: "persisted note",
      requestId: "73d22b7a-23c3-4bdb-a0b9-1bccb80a9e7b",
    };
  });
  await openSettings(page, "wallet");
  await page.getByRole("button", { name: "Transfer out" }).click();

  const checkPayment = page.getByRole("button", { name: "Check payment" });
  await expect(checkPayment).toBeEnabled();
  await expect(
    page.getByRole("textbox", { name: "Amount to send:" }),
  ).toBeDisabled();
  await checkPayment.click();

  await expect(
    page
      .locator("[data-sonner-toast]")
      .filter({ hasText: "Payment completed" }),
  ).toBeVisible();
  const commands = await page.evaluate(
    () =>
      (
        window as Window & {
          __BUZZ_E2E_COMMANDS__?: string[];
        }
      ).__BUZZ_E2E_COMMANDS__ ?? [],
  );
  expect(
    commands.filter((command) => command === "wallet_analyze_destination"),
  ).toHaveLength(0);
  expect(commands.filter((command) => command === "wallet_send")).toHaveLength(
    1,
  );
});

test("transfer out preserves a fresh request after a provider reconciliation error", async ({
  page,
}) => {
  await page.goto("/");
  await page.evaluate(() => {
    const testWindow = window as Window & {
      __BUZZ_E2E__?: {
        mock?: {
          walletSendErrors?: ({ code: string; message: string } | null)[];
          walletSendRequests?: Array<{
            destination: string;
            amount: number | null;
            message: string | null;
            requestId: string;
          }>;
        };
      };
    };
    if (!testWindow.__BUZZ_E2E__?.mock) {
      throw new Error("mock bridge config is unavailable");
    }
    testWindow.__BUZZ_E2E__.mock.walletSendErrors = [
      { code: "provider_error", message: "Provider sync failed" },
      null,
    ];
    testWindow.__BUZZ_E2E__.mock.walletSendRequests = [];
  });
  await openSettings(page, "wallet");
  await page.getByRole("button", { name: "Transfer out" }).click();

  await page.getByRole("textbox", { name: "Amount to send:" }).fill("500");
  await page
    .getByRole("textbox", { name: "Destination:" })
    .fill("alice@example.com");
  await page.getByRole("textbox", { name: "Note:" }).fill("fresh request");
  await page.getByRole("button", { name: "Send payment" }).click();

  await expect(
    page
      .locator("[data-sonner-toast]")
      .filter({ hasText: "Provider sync failed" }),
  ).toBeVisible();
  const checkPayment = page.getByRole("button", { name: "Check payment" });
  await expect(checkPayment).toBeEnabled();
  await expect(
    page.getByRole("textbox", { name: "Destination:" }),
  ).toBeDisabled();
  await checkPayment.click();
  await expect(
    page
      .locator("[data-sonner-toast]")
      .filter({ hasText: "Payment completed: ₿ 500" }),
  ).toBeVisible();

  const result = await page.evaluate(() => {
    const testWindow = window as Window & {
      __BUZZ_E2E__?: {
        mock?: {
          walletSendRequests?: unknown[];
        };
      };
      __BUZZ_E2E_COMMANDS__?: string[];
    };
    return {
      commands: testWindow.__BUZZ_E2E_COMMANDS__ ?? [],
      requests: testWindow.__BUZZ_E2E__?.mock?.walletSendRequests ?? [],
    };
  });
  expect(
    result.commands.filter(
      (command) => command === "wallet_analyze_destination",
    ),
  ).toHaveLength(1);
  expect(
    result.commands.filter((command) => command === "wallet_send"),
  ).toHaveLength(2);
  expect(result.requests).toHaveLength(2);
  expect(result.requests[1]).toEqual(result.requests[0]);
});

test("incoming payment event refreshes the wallet balance", async ({
  page,
}) => {
  await page.goto("/");
  await page.evaluate(() => {
    const testWindow = window as Window & {
      __BUZZ_E2E__?: { mock?: { walletTransactionDelayMs?: number } };
    };
    if (testWindow.__BUZZ_E2E__?.mock) {
      testWindow.__BUZZ_E2E__.mock.walletTransactionDelayMs = 1_000;
    }
  });
  await openSettings(page, "wallet");

  const balance = page.getByTestId("wallet-spendable-balance");
  await expect(balance).toContainText("20,000");

  await page.evaluate(async () => {
    const testWindow = window as Window & {
      __BUZZ_E2E__?: {
        mock?: {
          walletBalance?: number;
          walletSpendableBalance?: number;
        };
      };
    };
    if (!testWindow.__BUZZ_E2E__?.mock) {
      throw new Error("mock bridge config is unavailable");
    }
    testWindow.__BUZZ_E2E__.mock.walletBalance = 22_000;
    testWindow.__BUZZ_E2E__.mock.walletSpendableBalance = 22_000;
    const transaction = {
      id: "overview-incoming-payment",
      direction: "inbound",
      status: "completed",
      statusMessage: "Payment completed",
      amount: 2_000,
      fees: 0,
      note: null,
      payerNote: null,
      offerId: null,
      paymentHash: null,
      createdAtMs: Date.now(),
      finalizedAtMs: Date.now(),
    };
    await window.__BUZZ_E2E_EMIT_TAURI_EVENT__?.("wallet-incoming-payment", {
      transaction,
      status: {
        providerName: "Lexe",
        balance: 22_000,
        spendableBalance: 22_000,
        lightningBalance: 22_000,
        onchainBalance: 0,
      },
      transactions: [transaction],
    });
  });

  await expect(balance).toContainText("22,000", { timeout: 10_000 });
  await expect(
    page.getByTestId("wallet-transaction-overview-incoming-payment"),
  ).toBeVisible();
  // The delayed initial history response must not overwrite the newer event.
  await page.waitForTimeout(1_100);
  await expect(
    page.getByTestId("wallet-transaction-overview-incoming-payment"),
  ).toBeVisible();
  const commands = await page.evaluate(
    () =>
      (
        window as Window & {
          __BUZZ_E2E_COMMANDS__?: string[];
        }
      ).__BUZZ_E2E_COMMANDS__ ?? [],
  );
  expect(
    commands.filter((command) => command === "wallet_get_status").length,
  ).toBeGreaterThanOrEqual(1);
});

test("successful funding closes the QR and returns to the wallet overview", async ({
  page,
}) => {
  await page.goto("/");
  await openSettings(page, "wallet");

  await page.getByRole("button", { name: "Fund wallet" }).click();
  await expect(page.getByTestId("wallet-receive-qr")).toBeVisible();

  await page.evaluate(async () => {
    const testWindow = window as Window & {
      __BUZZ_E2E__?: {
        mock?: {
          walletBalance?: number;
          walletSpendableBalance?: number;
        };
      };
    };
    if (!testWindow.__BUZZ_E2E__?.mock) {
      throw new Error("mock bridge config is unavailable");
    }
    testWindow.__BUZZ_E2E__.mock.walletBalance = 22_000;
    testWindow.__BUZZ_E2E__.mock.walletSpendableBalance = 22_000;
    const transaction = {
      id: "new-funding-payment",
      direction: "inbound",
      status: "completed",
      statusMessage: "Payment completed",
      amount: 1_000,
      fees: 0,
      note: null,
      payerNote: null,
      offerId: null,
      paymentHash: null,
      createdAtMs: Date.now(),
      finalizedAtMs: Date.now(),
    };
    await window.__BUZZ_E2E_EMIT_TAURI_EVENT__?.("wallet-incoming-payment", {
      transaction,
      status: {
        providerName: "Lexe",
        balance: 22_000,
        spendableBalance: 22_000,
        lightningBalance: 22_000,
        onchainBalance: 0,
      },
      transactions: [transaction],
    });
  });

  await expect(page.getByTestId("wallet-receive-qr")).toHaveCount(0, {
    timeout: 10_000,
  });
  await expect(
    page.locator("[data-sonner-toast]").filter({ hasText: "Bitcoin received" }),
  ).toBeVisible();
  await expect(page.getByTestId("wallet-spendable-balance")).toHaveText(
    "₿ 22,000",
  );
  await expect(
    page.getByText("Transfer bitcoin from an external Lightning wallet"),
  ).toHaveCount(0);
  await expect(page.getByRole("button", { name: "Fund wallet" })).toBeVisible();
  await expect(
    page.getByRole("button", { name: "Transfer out" }),
  ).toBeVisible();
});

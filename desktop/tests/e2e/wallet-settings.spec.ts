import { expect, test } from "@playwright/test";

import { installMockBridge } from "../helpers/bridge";
import { FEATURE_OVERRIDES_STORAGE_KEY } from "../helpers/features";
import { openSettings } from "../helpers/settings";

test.beforeEach(async ({ page }) => {
  await page.addInitScript(
    ({ storageKey }) => {
      window.localStorage.setItem(
        storageKey,
        JSON.stringify({ bitcoin: true }),
      );
    },
    { storageKey: FEATURE_OVERRIDES_STORAGE_KEY },
  );
  await installMockBridge(page, {
    walletPollUpdates: [],
    walletBalance: 21_000,
    walletSpendableBalance: 20_000,
  });
});

test("profile menu shows the spendable balance and opens wallet settings", async ({
  page,
}) => {
  await page.goto("/");
  await page.getByTestId("sidebar-profile-avatar-button").click();

  const balance = page.getByTestId("profile-popover-wallet-balance");
  await expect(balance).toBeVisible();
  await expect(balance).toContainText("20,000");
  await expect(balance).toHaveAttribute(
    "aria-label",
    "Open wallet settings. Spendable balance ₿20,000",
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
  await expect(page.getByText("Reserved funds")).toHaveCount(0);
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

test("wallet polling refreshes the balance when Lexe reports a payment change", async ({
  page,
}) => {
  await page.goto("/");
  await openSettings(page, "wallet");

  const balance = page.getByTestId("wallet-spendable-balance");
  await expect(balance).toContainText("20,000");

  await page.evaluate(() => {
    const testWindow = window as Window & {
      __BUZZ_E2E__?: {
        mock?: {
          walletPollUpdates?: boolean[];
          walletBalance?: number;
          walletSpendableBalance?: number;
        };
      };
    };
    if (!testWindow.__BUZZ_E2E__?.mock) {
      throw new Error("mock bridge config is unavailable");
    }
    testWindow.__BUZZ_E2E__.mock.walletPollUpdates = [true];
    testWindow.__BUZZ_E2E__.mock.walletBalance = 22_000;
    testWindow.__BUZZ_E2E__.mock.walletSpendableBalance = 22_000;
  });

  await expect(balance).toContainText("22,000", { timeout: 10_000 });
  const commands = await page.evaluate(
    () =>
      (
        window as Window & {
          __BUZZ_E2E_COMMANDS__?: string[];
        }
      ).__BUZZ_E2E_COMMANDS__ ?? [],
  );
  expect(commands).toContain("wallet_poll_updates");
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

  await page.evaluate(() => {
    const testWindow = window as Window & {
      __BUZZ_E2E__?: {
        mock?: {
          walletPollUpdates?: boolean[];
          walletBalance?: number;
          walletSpendableBalance?: number;
        };
      };
    };
    if (!testWindow.__BUZZ_E2E__?.mock) {
      throw new Error("mock bridge config is unavailable");
    }
    testWindow.__BUZZ_E2E__.mock.walletPollUpdates = [true];
    testWindow.__BUZZ_E2E__.mock.walletBalance = 22_000;
    testWindow.__BUZZ_E2E__.mock.walletSpendableBalance = 22_000;
  });

  await expect(page.getByTestId("wallet-receive-qr")).toHaveCount(0, {
    timeout: 10_000,
  });
  await expect(
    page
      .locator("[data-sonner-toast]")
      .filter({ hasText: "Funds received: ₿ 1,000" }),
  ).toBeVisible();
  await expect(
    page.getByText("Transfer bitcoin from an external Lightning wallet"),
  ).toHaveCount(0);
  await expect(page.getByRole("button", { name: "Fund wallet" })).toBeVisible();
  await expect(
    page.getByRole("button", { name: "Transfer out" }),
  ).toBeVisible();
});

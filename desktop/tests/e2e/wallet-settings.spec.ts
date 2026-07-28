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

test("wallet balance exposes funding and transfer actions", async ({
  page,
}) => {
  await page.goto("/");
  await openSettings(page, "wallet");

  await expect(page.getByTestId("settings-wallet")).toBeVisible();
  await expect(page.getByTestId("wallet-reserved-funds")).toContainText(
    "1,000",
  );
  await expect(page.getByTestId("wallet-receive-qr")).toHaveCount(0);

  await page.getByRole("button", { name: "Fund wallet" }).click();

  await expect(page.getByTestId("wallet-receive-qr")).toBeVisible();
  await expect(
    page.getByText("Fund from an existing Lightning wallet."),
  ).toBeVisible();
  await expect(page.getByRole("button", { name: "Copy" })).toBeVisible();

  await page.getByRole("button", { name: "Transfer out" }).click();
  await expect(page.getByTestId("wallet-receive-qr")).toHaveCount(0);
  await expect(
    page.getByRole("textbox", { name: "Lightning destination" }),
  ).toBeVisible();
  await expect(
    page.getByRole("textbox", { name: "Bitcoin amount" }),
  ).toBeVisible();
  await expect(
    page.getByRole("textbox", { name: "Transfer note" }),
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
    page.getByText("Fund from an existing Lightning wallet."),
  ).toHaveCount(0);
  await expect(page.getByRole("button", { name: "Fund wallet" })).toBeVisible();
  await expect(
    page.getByRole("button", { name: "Transfer out" }),
  ).toBeVisible();
});

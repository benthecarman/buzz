import { expect, type Page, test } from "@playwright/test";

import { installMockBridge, TEST_IDENTITIES } from "../helpers/bridge";
import { FEATURE_OVERRIDES_STORAGE_KEY } from "../helpers/features";

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
});

async function openBobProfile(page: Page, bolt12Offer?: string | null) {
  await installMockBridge(page, {
    searchProfiles: [
      {
        pubkey: TEST_IDENTITIES.bob.pubkey,
        displayName: "bob",
        bolt12Offer,
      },
    ],
  });
  await page.goto("/");
  await page.getByTestId("channel-general").click();
  await expect(page.getByTestId("chat-title")).toHaveText("general");
  await page.getByTestId("channel-members-trigger").click();
  await expect(page.getByTestId("members-sidebar")).toBeVisible();
  await page
    .getByTestId(`sidebar-member-open-profile-${TEST_IDENTITIES.bob.pubkey}`)
    .click();
  await expect(page.getByTestId("user-profile-panel")).toBeVisible();
}

test("sends bitcoin without relying on kind-0 metadata", async ({ page }) => {
  await openBobProfile(page);

  await page.getByTestId("user-profile-send-bitcoin").click();
  const dialog = page.getByTestId("send-bitcoin-dialog");
  await expect(dialog).toBeVisible();

  const amount = dialog.getByLabel("Amount");
  await expect(amount).toBeEnabled();
  await expect(amount).toHaveValue("");
  await expect(amount).not.toHaveAttribute("placeholder");
  await expect(dialog.getByTestId("profile-bitcoin-amount-prefix")).toHaveText(
    "₿",
  );
  await expect(dialog.getByLabel("Comment")).toHaveAttribute(
    "placeholder",
    "optional",
  );
  await expect(
    dialog.getByTestId("profile-bitcoin-available-balance"),
  ).toHaveText("Available balance: ₿ 20,000");
  await expect(
    dialog.getByText("Pay bob's BOLT12 offer from your Buzz wallet."),
  ).toHaveCount(0);
  await amount.fill("21");
  await dialog.getByRole("button", { name: "Send bitcoin" }).click();

  await expect(dialog).toHaveCount(0);
  await expect(
    page.locator("[data-sonner-toast]").filter({ hasText: "₿ 21 sent" }),
  ).toBeVisible();
});

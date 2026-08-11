import { expect, test, type Page } from "@playwright/test";

import { installMockBridge } from "../helpers/bridge";

const AGENT_PUBKEY = "e".repeat(64);
const OWNER_PUBKEY = "f".repeat(64);

async function selectPaidAgent(page: Page, content: string) {
  const input = page.getByTestId("message-input");
  await input.fill(`${content} @quinn`);
  const suggestion = page.getByTestId(`mention-suggestion-${AGENT_PUBKEY}`);
  await expect(suggestion).toBeVisible();
  await suggestion.click();
  await page.keyboard.type("run");
}

async function runtimeTags(page: Page) {
  return page.evaluate(() => {
    const found: string[][] = [];
    const visit = (value: unknown) => {
      if (Array.isArray(value)) {
        if (value[0] === "agent_runtime") {
          found.push(value as string[]);
          return;
        }
        for (const item of value) visit(item);
        return;
      }
      if (typeof value === "string" && value.startsWith("[")) {
        try {
          visit(JSON.parse(value));
        } catch {}
        return;
      }
      if (value && typeof value === "object") {
        for (const item of Object.values(value)) visit(item);
      }
    };
    visit(window.__BUZZ_E2E_SIGNED_EVENTS__ ?? []);
    visit(window.__BUZZ_E2E_COMMAND_LOG__ ?? []);
    return found;
  });
}

async function paymentRequestCount(page: Page) {
  return page.evaluate(
    () =>
      (window.__BUZZ_E2E_COMMANDS__ ?? []).filter(
        (command) => command === "wallet_send_agent_runtime_zap",
      ).length,
  );
}

test("one zap grants reusable Agent access for five minutes", async ({
  page,
}) => {
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
  await installMockBridge(page, {
    relayAgents: [
      {
        pubkey: AGENT_PUBKEY,
        name: "quinn",
        ownerPubkey: OWNER_PUBKEY,
        priceSats: 255,
        respondTo: "anyone",
        channelNames: ["general"],
      },
    ],
    walletAgentRuntimeZapRequests: [],
  });
  await page.goto("/");
  await page.getByTestId("channel-general").click();

  await selectPaidAgent(page, "first");
  await page.getByTestId("send-message").click();
  await page
    .getByRole("alertdialog", { name: "Mention people outside this channel?" })
    .getByRole("button", { name: "Invite" })
    .click();
  const checkout = page.getByTestId("agent-runtime-checkout");
  await expect(checkout).toBeVisible();
  await expect(checkout).toContainText("₿ 255");
  await expect(checkout).toContainText("$0.20");
  await expect(checkout).toContainText("5 minutes of access");
  await expect(
    checkout.getByRole("button", { name: /^Pay ₿ 255/ }),
  ).toBeFocused();
  await page.keyboard.press("Enter");

  await expect
    .poll(async () => (await runtimeTags(page)).length)
    .toBeGreaterThan(0);
  const firstTag = (await runtimeTags(page))[0];
  expect(await paymentRequestCount(page)).toBe(1);
  await page.evaluate(() => {
    window.__BUZZ_E2E_SIGNED_EVENTS__ = [];
    window.__BUZZ_E2E_COMMAND_LOG__ = [];
    window.__BUZZ_E2E_CLEAR_AGENT_RUNTIME_ACCESS__?.();
  });

  await selectPaidAgent(page, "second");
  await page.getByTestId("send-message").click();
  await expect(checkout).toHaveCount(0);
  await expect
    .poll(async () => (await runtimeTags(page)).length)
    .toBeGreaterThan(0);
  const secondTag = (await runtimeTags(page))[0];
  expect(await paymentRequestCount(page)).toBe(1);
  expect(secondTag).toEqual(firstTag);

  await page.evaluate(() => {
    window.__BUZZ_E2E_EXPIRE_AGENT_RUNTIME_ACCESS__?.();
  });
  await selectPaidAgent(page, "third");
  await page.getByTestId("send-message").click();
  await expect(checkout).toBeVisible();
});

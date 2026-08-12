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

async function paymentBeginCount(page: Page) {
  return page.evaluate(
    () =>
      (window.__BUZZ_E2E_COMMANDS__ ?? []).filter(
        (command) => command === "wallet_begin_agent_runtime_zap",
      ).length,
  );
}

async function walletStatusRequestCount(page: Page) {
  return page.evaluate(
    () =>
      (window.__BUZZ_E2E_COMMANDS__ ?? []).filter(
        (command) => command === "wallet_get_status",
      ).length,
  );
}

async function sentPaymentAttemptIds(page: Page) {
  return page.evaluate(() =>
    (window.__BUZZ_E2E_COMMAND_PAYLOADS__ ?? [])
      .filter((entry) => entry.command === "wallet_send_agent_runtime_zap")
      .map(
        (entry) =>
          (
            entry.payload as {
              request?: { intentEventId?: string };
            }
          ).request?.intentEventId ?? "",
      ),
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
  const statusRequestsBeforeCheckout = await walletStatusRequestCount(page);

  await selectPaidAgent(page, "first");
  await page.getByTestId("send-message").click();
  await page
    .getByRole("alertdialog", { name: "Mention people outside this channel?" })
    .getByRole("button", { name: "Invite" })
    .click();
  const checkout = page.getByTestId("agent-runtime-checkout");
  await expect(checkout).toBeVisible();
  await expect
    .poll(() => walletStatusRequestCount(page))
    .toBe(statusRequestsBeforeCheckout + 1);
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
  await checkout.getByRole("button", { name: /^Pay ₿ 255/ }).click();
  await expect.poll(() => paymentRequestCount(page)).toBe(2);
  expect(await paymentBeginCount(page)).toBe(2);
  const attempts = await sentPaymentAttemptIds(page);
  expect(attempts).toHaveLength(2);
  expect(attempts[1]).not.toBe(attempts[0]);
});

test("paid Agent checkout shows structured wallet errors", async ({ page }) => {
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
    walletAgentRuntimeZapErrors: [
      {
        code: "offer_unavailable",
        message: "Agent has no active BOLT12 offer",
      },
    ],
  });
  await page.goto("/");
  await page.getByTestId("channel-general").click();

  await selectPaidAgent(page, "error");
  await page.getByTestId("send-message").click();
  await page
    .getByRole("alertdialog", { name: "Mention people outside this channel?" })
    .getByRole("button", { name: "Invite" })
    .click();

  const checkout = page.getByTestId("agent-runtime-checkout");
  await expect(checkout).toBeVisible();
  await checkout.getByRole("button", { name: /^Pay ₿ 255/ }).click();
  await expect(checkout).toContainText("Agent has no active BOLT12 offer");
});

test("unknown paid Agent result reuses its native intent", async ({ page }) => {
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
    walletAgentRuntimeZapErrors: [
      {
        code: "payment_status_unknown",
        message: "The payment result is still unknown",
      },
      null,
    ],
  });
  await page.goto("/");
  await page.getByTestId("channel-general").click();
  await selectPaidAgent(page, "retry");
  await page.getByTestId("send-message").click();
  await page
    .getByRole("alertdialog", { name: "Mention people outside this channel?" })
    .getByRole("button", { name: "Invite" })
    .click();

  const checkout = page.getByTestId("agent-runtime-checkout");
  const pay = checkout.getByRole("button", { name: /^Pay ₿ 255/ });
  await pay.click();
  await expect(checkout).toContainText("The payment result is still unknown");
  await pay.click();
  await expect(checkout).toHaveCount(0);
  expect(await paymentRequestCount(page)).toBe(2);
  expect(await paymentBeginCount(page)).toBe(1);
  expect(new Set(await sentPaymentAttemptIds(page)).size).toBe(1);
});

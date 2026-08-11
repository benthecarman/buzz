import { expect, type Page, test } from "@playwright/test";

import { installMockBridge, TEST_IDENTITIES } from "../helpers/bridge";
import { FEATURE_OVERRIDES_STORAGE_KEY } from "../helpers/features";

async function renderedTextContrast(
  locator: import("@playwright/test").Locator,
): Promise<number> {
  return locator.evaluate((element) => {
    type Rgba = { r: number; g: number; b: number; a: number };

    const canvas = document.createElement("canvas");
    canvas.width = 1;
    canvas.height = 1;
    const context = canvas.getContext("2d", { willReadFrequently: true });
    if (!context) throw new Error("Could not create contrast test canvas");

    const parseColor = (color: string): Rgba => {
      context.clearRect(0, 0, 1, 1);
      context.fillStyle = color;
      context.fillRect(0, 0, 1, 1);
      const [r, g, b, alpha] = context.getImageData(0, 0, 1, 1).data;
      return { r, g, b, a: alpha / 255 };
    };
    const composite = (foreground: Rgba, background: Rgba): Rgba => {
      const alpha = foreground.a + background.a * (1 - foreground.a);
      if (alpha === 0) return { r: 0, g: 0, b: 0, a: 0 };

      return {
        r:
          (foreground.r * foreground.a +
            background.r * background.a * (1 - foreground.a)) /
          alpha,
        g:
          (foreground.g * foreground.a +
            background.g * background.a * (1 - foreground.a)) /
          alpha,
        b:
          (foreground.b * foreground.a +
            background.b * background.a * (1 - foreground.a)) /
          alpha,
        a: alpha,
      };
    };

    const backgroundLayers: Rgba[] = [];
    let current: Element | null = element;
    while (current) {
      backgroundLayers.push(
        parseColor(window.getComputedStyle(current).backgroundColor),
      );
      current = current.parentElement;
    }
    let background: Rgba = { r: 255, g: 255, b: 255, a: 1 };
    for (const layer of backgroundLayers.reverse()) {
      background = composite(layer, background);
    }
    const foreground = composite(
      parseColor(window.getComputedStyle(element).color),
      background,
    );
    const luminance = (color: Rgba) => {
      const [r, g, b] = [color.r, color.g, color.b].map((value) => {
        const channel = value / 255;
        return channel <= 0.04045
          ? channel / 12.92
          : ((channel + 0.055) / 1.055) ** 2.4;
      });
      return 0.2126 * r + 0.7152 * g + 0.0722 * b;
    };
    const foregroundLuminance = luminance(foreground);
    const backgroundLuminance = luminance(background);
    return (
      (Math.max(foregroundLuminance, backgroundLuminance) + 0.05) /
      (Math.min(foregroundLuminance, backgroundLuminance) + 0.05)
    );
  });
}

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
  const comment = dialog.getByLabel("Comment");
  await expect(comment).not.toHaveAttribute("placeholder");
  const commentAnnotation = dialog.getByTestId(
    "profile-bitcoin-comment-annotation",
  );
  await expect(commentAnnotation).toHaveText("(Optional)");
  await expect(commentAnnotation).toHaveClass(
    "text-xs text-muted-foreground/70",
  );
  await expect(dialog).toHaveCSS("max-width", "320px");
  const cancelButton = dialog.getByRole("button", {
    exact: true,
    name: "Cancel",
  });
  const sendButton = dialog.getByRole("button", { exact: true, name: "Send" });
  const [amountBox, commentBox, cancelButtonBox, sendButtonBox] =
    await Promise.all([
      amount.boundingBox(),
      comment.boundingBox(),
      cancelButton.boundingBox(),
      sendButton.boundingBox(),
    ]);
  if (!amountBox || !commentBox || !cancelButtonBox || !sendButtonBox) {
    throw new Error("Send bitcoin controls must be visible");
  }
  expect(amountBox.width).toBeGreaterThan(160);
  expect(commentBox.width).toBeGreaterThan(160);
  expect(
    Math.abs(
      amountBox.x + amountBox.width - (sendButtonBox.x + sendButtonBox.width),
    ),
  ).toBeLessThan(2);
  expect(
    Math.abs(
      commentBox.x + commentBox.width - (sendButtonBox.x + sendButtonBox.width),
    ),
  ).toBeLessThan(2);
  expect(sendButtonBox.width).toBeCloseTo(cancelButtonBox.width, 0);
  await expect(
    dialog.getByText("Pay bob's BOLT12 offer from your Buzz wallet."),
  ).toHaveCount(0);
  await amount.fill("21");
  await sendButton.click();

  await expect(dialog).toHaveCount(0);
  await expect(
    page.locator("[data-sonner-toast]").filter({ hasText: "₿ 21 sent" }),
  ).toBeVisible();
});

test("shows the received amount in the incoming payment toast", async ({
  page,
}) => {
  await installMockBridge(page);
  await page.goto("/");
  await expect(page.getByTestId("channel-general")).toBeVisible();

  await page.evaluate(async () => {
    await window.__BUZZ_E2E_EMIT_TAURI_EVENT__?.("wallet-incoming-payment", {
      id: "incoming-payment",
      direction: "inbound",
      status: "completed",
      statusMessage: "Payment completed",
      amount: 49,
      fees: 0,
      note: null,
      payerNote: null,
      offerId: null,
      createdAtMs: Date.now(),
      finalizedAtMs: Date.now(),
    });
  });

  const toast = page
    .locator("[data-sonner-toast]")
    .filter({ hasText: "Bitcoin received" });
  await expect(toast).toBeVisible();
  await expect(toast).toContainText("₿ 49");
});

test("message zap sends ₿50 optimistically without progress chrome", async ({
  page,
}) => {
  await page.addInitScript(() => {
    window.localStorage.setItem("buzz-theme", "buzz-dark");
  });
  await installMockBridge(page, {
    walletProfileZapDelayMs: 1_000,
    walletProfileZapStatus: "completed",
  });
  await page.goto("/");
  await page.getByTestId("channel-general").click();
  await expect(page.getByTestId("chat-title")).toHaveText("general");
  await expect
    .poll(() =>
      page.evaluate(
        () =>
          window.__BUZZ_E2E_HAS_MOCK_LIVE_SUBSCRIPTION__?.({
            channelName: "general",
          }) ?? false,
      ),
    )
    .toBe(true);
  const message = await page.evaluate((bobPubkey) => {
    return window.__BUZZ_E2E_EMIT_MOCK_MESSAGE__?.({
      channelName: "general",
      content: "Zap this pending payment",
      id: "d4".repeat(32),
      pubkey: bobPubkey,
    });
  }, TEST_IDENTITIES.bob.pubkey);
  if (!message) {
    throw new Error("Mock message emitter is not installed");
  }
  await page.evaluate(
    ({ messageId, reactorPubkey }) => {
      window.__BUZZ_E2E_EMIT_MOCK_MESSAGE__?.({
        channelName: "general",
        content: "👍",
        extraTags: [["e", messageId]],
        kind: 7,
        pubkey: reactorPubkey,
      });
    },
    { messageId: message.id, reactorPubkey: TEST_IDENTITIES.alice.pubkey },
  );

  const messageRow = page.locator(`[data-message-id="${message.id}"]`);
  const zapAction = messageRow.getByTestId(`zap-message-${message.id}`);
  await messageRow.hover();
  await expect(zapAction).toBeVisible();
  await expect(zapAction).toHaveAccessibleName("Zap ₿50");
  await expect(zapAction.locator("img")).toBeVisible();
  await expect(zapAction).toHaveText("");
  await zapAction.hover();
  await expect(
    page.getByRole("tooltip", {
      name: "Zap ₿50",
    }),
  ).toBeVisible();
  await zapAction.click();
  await expect(page.getByTestId("send-bitcoin-dialog")).toHaveCount(0);

  const optimisticZap = messageRow.getByTestId("message-zap");
  const ordinaryReaction = page.getByRole("button", {
    name: "Toggle 👍 reaction",
  });
  await expect(optimisticZap).toBeVisible();
  await expect(optimisticZap).toContainText("50");
  await expect(optimisticZap).toHaveAccessibleName("₿ 50 across 1 zap");
  await expect(optimisticZap.locator(".animate-spin")).toHaveCount(0);
  await expect(optimisticZap).not.toHaveClass(/\bborder-dashed\b/);
  await expect(ordinaryReaction).toBeVisible();
  await expect(page.locator("html")).toHaveClass(/dark/);
  await expect(optimisticZap).toHaveClass(/\bborder-border\/70\b/);
  await expect(optimisticZap).toHaveClass(/\bbg-muted\/70\b/);
  await expect(optimisticZap).toHaveClass(/\btext-foreground\/90\b/);
  await expect(optimisticZap).not.toHaveClass(/\bborder-blue-200\b/);
  await expect(optimisticZap).not.toHaveClass(/\bbg-white\b/);
  const [zapBorderColor, reactionBorderColor] = await Promise.all(
    [optimisticZap, ordinaryReaction].map((locator) =>
      locator.evaluate((element) => getComputedStyle(element).borderTopColor),
    ),
  );
  expect(zapBorderColor).toBe(reactionBorderColor);
  expect(await renderedTextContrast(optimisticZap)).toBeGreaterThanOrEqual(4.5);

  await expect
    .poll(() =>
      page.evaluate(
        () =>
          window.__BUZZ_E2E_COMMAND_PAYLOADS__?.some(
            (candidate) => candidate.command === "wallet_send_profile_zap",
          ) ?? false,
      ),
    )
    .toBe(true);
  const request = await page.evaluate(() => {
    return (
      window.__BUZZ_E2E_COMMAND_PAYLOADS__?.find(
        (candidate) => candidate.command === "wallet_send_profile_zap",
      )?.payload ?? null
    );
  });
  expect(request).toEqual({
    request: {
      amount: 50,
      comment: null,
      idempotencyKey: expect.any(String),
      recipientPubkey: TEST_IDENTITIES.bob.pubkey,
      targetEventId: message.id,
      targetEventKind: 9,
    },
  });

  await expect(zapAction).toBeDisabled();
  await page.waitForTimeout(1_100);
  await expect(page.locator("[data-sonner-toast]")).toHaveCount(0);
});

test("split-pane message copies share one in-flight zap", async ({ page }) => {
  await installMockBridge(page, {
    walletProfileZapDelayMs: 1_000,
    walletProfileZapStatus: "completed",
  });
  await page.goto("/");
  await page.getByTestId("channel-general").click();
  await expect(page.getByTestId("chat-title")).toHaveText("general");
  await expect
    .poll(() =>
      page.evaluate(
        () =>
          window.__BUZZ_E2E_HAS_MOCK_LIVE_SUBSCRIPTION__?.({
            channelName: "general",
          }) ?? false,
      ),
    )
    .toBe(true);
  const message = await page.evaluate((bobPubkey) => {
    const emit = window.__BUZZ_E2E_EMIT_MOCK_MESSAGE__;
    if (!emit) return null;
    const root = emit({
      channelName: "general",
      content: "Zap this split-pane message",
      id: "d7".repeat(32),
      pubkey: bobPubkey,
    });
    emit({
      channelName: "general",
      content: "Open the thread copy",
      parentEventId: root.id,
      pubkey: bobPubkey,
    });
    return root;
  }, TEST_IDENTITIES.bob.pubkey);
  if (!message) {
    throw new Error("Mock message emitter is not installed");
  }

  const timelineRow = page
    .getByTestId("message-timeline")
    .locator(`[data-message-id="${message.id}"]`);
  await timelineRow.hover();
  await timelineRow.getByRole("button", { name: "Reply" }).click();
  await expect(page.getByTestId("message-thread-panel")).toBeVisible();

  const zapActions = page.getByTestId(`zap-message-${message.id}`);
  await expect(zapActions).toHaveCount(2);
  await Promise.all([
    zapActions.nth(0).click({ force: true }),
    zapActions.nth(1).click({ force: true }),
  ]);

  await expect
    .poll(() =>
      page.evaluate(
        () =>
          (window.__BUZZ_E2E_COMMAND_PAYLOADS__ ?? []).filter(
            (candidate) => candidate.command === "wallet_send_profile_zap",
          ).length,
      ),
    )
    .toBe(1);
  await page.waitForTimeout(1_100);
  const requests = await page.evaluate(() =>
    (window.__BUZZ_E2E_COMMAND_PAYLOADS__ ?? [])
      .filter((candidate) => candidate.command === "wallet_send_profile_zap")
      .map((candidate) => candidate.payload),
  );
  expect(requests).toHaveLength(1);
  expect(requests[0]).toEqual({
    request: {
      amount: 50,
      comment: null,
      idempotencyKey: expect.any(String),
      recipientPubkey: TEST_IDENTITIES.bob.pubkey,
      targetEventId: message.id,
      targetEventKind: 9,
    },
  });
});

test("message zap rolls back and shows a toast when payment fails", async ({
  page,
}) => {
  await installMockBridge(page, {
    walletProfileZapDelayMs: 500,
    walletProfileZapStatus: "failed",
  });
  await page.goto("/");
  await page.getByTestId("channel-general").click();
  await expect(page.getByTestId("chat-title")).toHaveText("general");
  await expect
    .poll(() =>
      page.evaluate(
        () =>
          window.__BUZZ_E2E_HAS_MOCK_LIVE_SUBSCRIPTION__?.({
            channelName: "general",
          }) ?? false,
      ),
    )
    .toBe(true);
  const message = await page.evaluate((bobPubkey) => {
    return window.__BUZZ_E2E_EMIT_MOCK_MESSAGE__?.({
      channelName: "general",
      content: "Roll back this failed zap",
      id: "d5".repeat(32),
      pubkey: bobPubkey,
    });
  }, TEST_IDENTITIES.bob.pubkey);
  if (!message) {
    throw new Error("Mock message emitter is not installed");
  }

  const messageRow = page.locator(`[data-message-id="${message.id}"]`);
  const zapAction = messageRow.getByTestId(`zap-message-${message.id}`);
  await zapAction.click({ force: true });
  await expect(messageRow.getByTestId("message-zap")).toContainText("50");

  await expect(
    page.locator("[data-sonner-toast]").filter({ hasText: "Payment failed" }),
  ).toBeVisible();
  await expect(messageRow.getByTestId("message-zap")).toHaveCount(0);
  await expect(zapAction).toBeEnabled();
});

test("message zap survives a provider outage before pending payment fails", async ({
  page,
}) => {
  await installMockBridge(page, {
    walletProfileZapStatuses: ["pending", "failed"],
    walletProfileZapErrors: [
      null,
      { code: "provider_error", message: "Provider temporarily unavailable" },
      null,
    ],
  });
  await page.goto("/");
  await page.getByTestId("channel-general").click();
  await expect(page.getByTestId("chat-title")).toHaveText("general");
  await expect
    .poll(() =>
      page.evaluate(
        () =>
          window.__BUZZ_E2E_HAS_MOCK_LIVE_SUBSCRIPTION__?.({
            channelName: "general",
          }) ?? false,
      ),
    )
    .toBe(true);
  const message = await page.evaluate((bobPubkey) => {
    return window.__BUZZ_E2E_EMIT_MOCK_MESSAGE__?.({
      channelName: "general",
      content: "Reconcile this pending zap",
      id: "d6".repeat(32),
      pubkey: bobPubkey,
    });
  }, TEST_IDENTITIES.bob.pubkey);
  if (!message) {
    throw new Error("Mock message emitter is not installed");
  }

  const messageRow = page.locator(`[data-message-id="${message.id}"]`);
  const zapAction = messageRow.getByTestId(`zap-message-${message.id}`);
  await zapAction.click({ force: true });
  await expect(messageRow.getByTestId("message-zap")).toContainText("50");
  await expect(zapAction).toBeDisabled();
  await expect(page.locator("[data-sonner-toast]")).toHaveCount(0);

  await expect
    .poll(() =>
      page.evaluate(
        () =>
          (window.__BUZZ_E2E_COMMAND_PAYLOADS__ ?? []).filter(
            (candidate) => candidate.command === "wallet_send_profile_zap",
          ).length,
      ),
    )
    .toBe(2);
  await expect(messageRow.getByTestId("message-zap")).toContainText("50");
  await expect(zapAction).toBeDisabled();
  await expect(page.locator("[data-sonner-toast]")).toHaveCount(0);

  await expect(
    page.locator("[data-sonner-toast]").filter({ hasText: "Payment failed" }),
  ).toBeVisible();
  await expect(messageRow.getByTestId("message-zap")).toHaveCount(0);
  await expect(zapAction).toBeEnabled();

  const requests = await page.evaluate(() =>
    (window.__BUZZ_E2E_COMMAND_PAYLOADS__ ?? [])
      .filter((candidate) => candidate.command === "wallet_send_profile_zap")
      .map((candidate) => candidate.payload),
  );
  expect(requests).toHaveLength(3);
  expect(requests.slice(1)).toEqual([requests[0], requests[0]]);
});

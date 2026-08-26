import { expect, test } from "@playwright/test";

import { installMockBridge, TEST_IDENTITIES } from "../helpers/bridge";

// Per-agent budget editing in the agent dialog's advanced settings.

const AGENT_PUBKEY = TEST_IDENTITIES.tyler.pubkey;
const AGENT_NAME = "Tyler Agent";
const PERSONA_ID = "custom:wallet-budget-agent";

test.beforeEach(async ({ page }) => {
  await installMockBridge(page, {
    managedAgents: [
      {
        pubkey: AGENT_PUBKEY,
        name: AGENT_NAME,
        status: "stopped",
        channelNames: ["agents"],
        respondTo: "anyone",
      },
    ],
    walletDefaultNwcPolicy: {
      mode: "budget",
      budgetAmount: 1_000,
      budgetPeriod: "day",
    },
  });
});

test("agent edit dialog edits the wallet spending policy", async ({ page }) => {
  await page.goto("/");
  await page.getByTestId("open-agents-view").click();

  const agentButton = page.getByRole("button", {
    name: `${AGENT_NAME} agent profile`,
  });
  await expect(agentButton).toBeVisible({ timeout: 10_000 });
  await agentButton.click();

  await expect(page.getByTestId("user-profile-panel")).toBeVisible({
    timeout: 10_000,
  });
  await page.getByTestId("user-profile-edit-agent").click();

  await expect(page.getByTestId("edit-agent-dialog")).toBeVisible({
    timeout: 10_000,
  });
  // Provider field visible = runtime catalog loaded and form settled.
  await expect(page.locator("#edit-agent-llm-provider")).toBeVisible({
    timeout: 10_000,
  });

  const section = page.getByTestId("edit-agent-wallet-spending");
  await expect(section).toHaveCount(0);

  // The model field remains in the main form. Wallet spending does not.
  await expect(page.locator("#edit-agent-model")).toBeVisible();
  await page.getByRole("button", { name: "Advanced", exact: true }).click();

  await expect(section).toBeVisible({ timeout: 10_000 });
  await expect(section).toContainText(
    "Every payment waits for your approval. Balance requests return zero.",
  );
  await page
    .getByTestId(`edit-agent-spending-mode-${AGENT_PUBKEY}-budget`)
    .click();
  await section
    .getByRole("textbox", { name: `Budget for ${AGENT_NAME}` })
    .fill("2500");
  await page
    .getByTestId(`edit-agent-spending-period-${AGENT_PUBKEY}-week`)
    .click();
  await section.getByRole("button", { name: "Save" }).click();

  await expect(section).toContainText("₿ 2,500 left");
  await expect(
    page
      .locator("[data-sonner-toast]")
      .filter({ hasText: `${AGENT_NAME}'s wallet policy was updated` }),
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
    commands.filter((command) => command === "wallet_set_nwc_policy"),
  ).toHaveLength(1);
});

test("linked agent edit paths show the wallet spending policy", async ({
  page,
}) => {
  const personaName = "Wallet Budget Agent";
  await installMockBridge(page, {
    managedAgents: [
      {
        pubkey: AGENT_PUBKEY,
        name: AGENT_NAME,
        personaId: PERSONA_ID,
        status: "stopped",
        channelNames: ["agents"],
      },
    ],
    personas: [
      {
        id: PERSONA_ID,
        displayName: personaName,
        systemPrompt: "Stay within the wallet budget.",
      },
    ],
    walletNwcClients: [
      {
        agentPubkey: AGENT_PUBKEY,
        agentName: AGENT_NAME,
        mode: "budget",
        budgetAmount: 1_800,
        budgetPeriod: "day",
        spentAmount: 200,
        remainingAmount: 1_600,
        periodEndsAtMs: Date.now() + 86_400_000,
      },
    ],
    walletSpendableBalance: 2_675,
  });
  await page.route(
    "https://api.coinbase.com/v2/prices/BTC-USD/spot",
    async (route) => {
      await route.fulfill({
        body: JSON.stringify({
          data: { amount: "100000.00", currency: "USD" },
        }),
        contentType: "application/json",
        status: 200,
      });
    },
  );

  await page.goto("/");
  await page.getByTestId("open-agents-view").click();
  await page.getByLabel(`Open actions for ${personaName}`).click();
  await page.getByRole("menuitem", { name: "Edit" }).click();

  await expect(page.getByTestId("persona-dialog")).toBeVisible({
    timeout: 10_000,
  });
  const section = page.getByTestId("edit-agent-wallet-spending");
  await expect(section).toHaveCount(0);
  await page.getByRole("button", { name: "Advanced", exact: true }).click();
  await expect(section).toBeVisible({ timeout: 10_000 });
  await expect(
    section.getByRole("textbox", { name: `Budget for ${AGENT_NAME}` }),
  ).toHaveValue("1800");
  await expect(section).toContainText("Budget (of total: ₿2,675)");
  await expect(
    page.getByTestId(`edit-agent-spending-budget-${AGENT_PUBKEY}-usd`),
  ).toHaveText("$1.80");

  await page.getByRole("button", { name: "Cancel", exact: true }).click();
  await page
    .getByRole("button", { name: `${personaName} agent profile` })
    .click();
  await expect(page.getByTestId("user-profile-panel")).toBeVisible();
  await page.getByTestId("user-profile-edit-agent").click();
  await expect(page.getByTestId("persona-dialog")).toBeVisible();
  await page.getByRole("button", { name: "Advanced", exact: true }).click();
  await expect(section).toBeVisible();
});

test("create agent overrides the default wallet budget", async ({ page }) => {
  const agentName = `Budget agent ${Date.now()}`;

  await page.goto("/");
  await page.getByTestId("open-agents-view").click();
  await page.getByTestId("new-agent-card").click();
  await page.locator("#persona-display-name").fill(agentName);
  await page.getByRole("tab", { name: "Customize for this agent" }).click();
  const provider = page.locator("#persona-llm-provider");
  await expect(provider).toBeVisible({ timeout: 10_000 });
  await provider.press("Enter");
  await page
    .getByRole("menuitemradio", {
      exact: true,
      name: "Buzz shared compute",
    })
    .click();
  await page.getByRole("button", { name: "Advanced", exact: true }).click();

  const section = page.getByTestId("create-agent-wallet-spending");
  await expect(section).toBeVisible({ timeout: 10_000 });
  await expect(
    page.getByTestId("create-agent-spending-mode-budget"),
  ).toHaveAttribute("aria-pressed", "true");
  await section
    .getByRole("textbox", { name: "Budget for this agent" })
    .fill("3200");
  await page.getByTestId("create-agent-spending-period-week").click();

  await expect(page.getByTestId("persona-dialog-submit")).toBeEnabled();
  await page.getByTestId("persona-dialog-submit").click();
  await expect(page.getByRole("dialog")).toHaveCount(0, { timeout: 10_000 });

  const policyUpdate = await page.evaluate((name) => {
    const log = (
      window as Window & {
        __BUZZ_E2E_COMMAND_LOG__?: Array<{
          command: string;
          payload: unknown;
        }>;
      }
    ).__BUZZ_E2E_COMMAND_LOG__;
    const created = log
      ?.filter((entry) => entry.command === "create_managed_agent")
      .map((entry) => entry.payload as { input?: { name?: string } })
      .find((payload) => payload.input?.name === name);
    if (!created) return null;
    return log
      ?.filter((entry) => entry.command === "wallet_set_nwc_policy")
      .map(
        (entry) =>
          entry.payload as {
            update?: {
              mode?: string;
              budgetAmount?: number;
              budgetPeriod?: string;
            };
          },
      )
      .at(-1)?.update;
  }, agentName);

  expect(policyUpdate).toMatchObject({
    mode: "budget",
    budgetAmount: 3_200,
    budgetPeriod: "week",
  });
});

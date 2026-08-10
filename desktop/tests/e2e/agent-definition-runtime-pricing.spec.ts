import { expect, test } from "@playwright/test";

import { installMockBridge, TEST_IDENTITIES } from "../helpers/bridge";
import { FEATURE_OVERRIDES_STORAGE_KEY } from "../helpers/features";

/**
 * Paid runtime is edited from the definition dialog — the surface owners
 * actually open from the Agents view — but the rate itself lives on the live
 * instances (`personaRuntimePricing.ts`). These tests pin both halves: the
 * control reads the instances' stored rate, and Save writes it back to them
 * along with the access mode that makes it applicable.
 */
const PERSONA_ID = "persona-runtime-pricing-e2e";
const PERSONA_NAME = "Priced Agent";
const AGENT_PUBKEY = TEST_IDENTITIES.tyler.pubkey;

// Satisfies the dialog's credential gate so Save reflects pricing validity
// alone rather than a missing provider key.
const BAKED_DEFAULTS = [
  { key: "BUZZ_AGENT_PROVIDER", value: "anthropic", masked: false },
  { key: "BUZZ_AGENT_MODEL", value: "claude-opus-4-8", masked: false },
  { key: "ANTHROPIC_API_KEY", value: "sk-ant-baked-test", masked: true },
];

/**
 * Paid runtime needs the wallet that mints the agent's payment offer, so the
 * control only exists once the Bitcoin preview feature is on. Must run before
 * `installMockBridge` — React reads the override on mount.
 */
async function enableWalletFeature(page: import("@playwright/test").Page) {
  await page.addInitScript((key: string) => {
    window.localStorage.setItem(key, JSON.stringify({ bitcoin: true }));
  }, FEATURE_OVERRIDES_STORAGE_KEY);
}

/**
 * Never re-navigates: `installMockBridge` seeds through an init script, so a
 * reload would reset the mock store and hide whether a save persisted.
 */
async function openDefinitionAdvanced(page: import("@playwright/test").Page) {
  await page
    .getByRole("button", { name: `Open actions for ${PERSONA_NAME}` })
    .click();
  await page.getByRole("menuitem", { name: "Edit" }).click();

  const dialog = page.getByTestId("persona-dialog");
  await expect(dialog).toBeVisible({ timeout: 10_000 });
  await dialog.getByRole("button", { name: "Advanced", exact: true }).click();
  return dialog;
}

async function gotoAgents(page: import("@playwright/test").Page) {
  await page.goto("/");
  await page.getByTestId("open-agents-view").click();
  await expect(page.getByTestId("agents-library-personas")).toBeVisible({
    timeout: 10_000,
  });
}

test.describe("agent definition runtime pricing", () => {
  test("prices a live instance from the definition dialog", async ({
    page,
  }) => {
    await enableWalletFeature(page);
    await installMockBridge(page, {
      bakedBuildEnv: BAKED_DEFAULTS,
      personas: [
        {
          id: PERSONA_ID,
          displayName: PERSONA_NAME,
          systemPrompt: "A definition with one live instance.",
        },
      ],
      managedAgents: [
        {
          pubkey: AGENT_PUBKEY,
          name: PERSONA_NAME,
          personaId: PERSONA_ID,
          status: "running",
          channelNames: ["agents"],
        },
      ],
    });

    await gotoAgents(page);
    const dialog = await openDefinitionAdvanced(page);

    // Owner-only access cannot take payment: the control explains itself
    // rather than disappearing.
    const paymentToggle = dialog.getByRole("checkbox", {
      name: "Require payment for runtime",
    });
    await expect(paymentToggle).toBeVisible();
    await expect(paymentToggle).toBeDisabled();
    await expect(
      dialog.getByText("Applies to this agent's 1 running instance"),
    ).toBeVisible();

    await dialog.locator("#agent-respond-to").click();
    await page.getByRole("menuitemradio", { name: "Anyone" }).click();

    await expect(paymentToggle).toBeEnabled();
    await paymentToggle.check();
    await dialog.locator("#agent-runtime-price").fill("21");
    await dialog.getByRole("button", { name: /^Save/ }).click();
    await expect(dialog).not.toBeVisible({ timeout: 10_000 });

    // The rate round-trips through the instance, not the definition. No
    // reload here — see openDefinitionAdvanced.
    const reopened = await openDefinitionAdvanced(page);
    await expect(reopened.locator("#agent-respond-to")).toContainText("Anyone");
    const reopenedToggle = reopened.getByRole("checkbox", {
      name: "Require payment for runtime",
    });
    await expect(reopenedToggle).toBeChecked();
    await expect(reopened.locator("#agent-runtime-price")).toHaveValue("21");

    // Turning it off clears the instance's rate.
    await reopenedToggle.uncheck();
    await reopened.getByRole("button", { name: /^Save/ }).click();
    await expect(reopened).not.toBeVisible({ timeout: 10_000 });

    const cleared = await openDefinitionAdvanced(page);
    await expect(
      cleared.getByRole("checkbox", { name: "Require payment for runtime" }),
    ).not.toBeChecked();
    await expect(cleared.locator("#agent-runtime-price")).toHaveCount(0);
  });

  test("explains that a definition with no live instance cannot be priced", async ({
    page,
  }) => {
    await enableWalletFeature(page);
    await installMockBridge(page, {
      bakedBuildEnv: BAKED_DEFAULTS,
      personas: [
        {
          id: PERSONA_ID,
          displayName: PERSONA_NAME,
          systemPrompt: "A definition nobody has started yet.",
          respondTo: "anyone",
        },
      ],
      managedAgents: [],
    });

    await gotoAgents(page);
    const dialog = await openDefinitionAdvanced(page);

    await expect(
      dialog.getByRole("checkbox", { name: "Require payment for runtime" }),
    ).toBeDisabled();
    await expect(
      dialog.getByText(
        "Start this agent in a community before setting a rate",
        {
          exact: false,
        },
      ),
    ).toBeVisible();
  });

  test("prices an instance that already answers outsiders", async ({
    page,
  }) => {
    await enableWalletFeature(page);
    await installMockBridge(page, {
      bakedBuildEnv: BAKED_DEFAULTS,
      personas: [
        {
          id: PERSONA_ID,
          displayName: PERSONA_NAME,
          systemPrompt: "A definition whose default access stayed owner-only.",
        },
      ],
      managedAgents: [
        {
          pubkey: AGENT_PUBKEY,
          name: PERSONA_NAME,
          personaId: PERSONA_ID,
          status: "running",
          channelNames: ["agents"],
          respondTo: "allowlist",
          respondToAllowlist: [TEST_IDENTITIES.alice.pubkey],
        },
      ],
    });

    await gotoAgents(page);
    const dialog = await openDefinitionAdvanced(page);

    // The definition default reads "Only me", but the running instance already
    // answers an allowlist — so it can be charged without changing its access.
    await expect(dialog.locator("#agent-respond-to")).toContainText("Only me");
    const paymentToggle = dialog.getByRole("checkbox", {
      name: "Require payment for runtime",
    });
    await expect(paymentToggle).toBeEnabled();
    await paymentToggle.check();
    await dialog.locator("#agent-runtime-price").fill("7");
    await dialog.getByRole("button", { name: /^Save/ }).click();
    await expect(dialog).not.toBeVisible({ timeout: 10_000 });

    const reopened = await openDefinitionAdvanced(page);
    await expect(reopened.locator("#agent-runtime-price")).toHaveValue("7");
    // The instance keeps the access it had; only the rate changed.
    await expect(reopened.locator("#agent-respond-to")).toContainText(
      "Only me",
    );
  });
});

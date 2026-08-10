import assert from "node:assert/strict";
import test from "node:test";

import {
  accessTakesPayment,
  derivePersonaRuntimePricing,
  instancesNeedingAccessLift,
  personaLiveInstances,
  personaRuntimePricingUpdates,
  pricingAppliesToInstances,
} from "./personaRuntimePricing.ts";

// ── Definition-dialog pricing projection ────────────────────────────────────
//
// The rate is instance state edited from the definition dialog, so the read
// (instances → control) and the write (control → instances) are the seam that
// must not drift. A write that carries a rate without the access it needs is
// rejected by `validate_runtime_price` in the backend, which is why enabling a
// rate also aligns each instance's access with the draft.

const ALICE = "a".repeat(64);
const BOB = "b".repeat(64);

function instance(overrides) {
  return {
    pubkey: "instance",
    personaId: "persona-1",
    respondTo: "owner-only",
    respondToAllowlist: [],
    pricePerMinuteSats: null,
    ...overrides,
  };
}

test("only allowlist and anyone access can carry a rate", () => {
  assert.equal(accessTakesPayment("allowlist"), true);
  assert.equal(accessTakesPayment("anyone"), true);
  assert.equal(accessTakesPayment("owner-only"), false);
});

test("live instances are scoped to the edited definition", () => {
  const mine = instance({ pubkey: "mine" });
  const theirs = instance({ pubkey: "theirs", personaId: "persona-2" });

  assert.deepEqual(personaLiveInstances([mine, theirs], "persona-1"), [mine]);
  assert.deepEqual(personaLiveInstances([mine, theirs], undefined), []);
});

test("unpriced instances leave the control off", () => {
  assert.deepEqual(derivePersonaRuntimePricing([instance({})]), {
    enabled: false,
    price: "",
    mixed: false,
  });
});

test("matching rates report a single price, differing rates report mixed", () => {
  assert.deepEqual(
    derivePersonaRuntimePricing([
      instance({ pubkey: "a", pricePerMinuteSats: 20 }),
      instance({ pubkey: "b", pricePerMinuteSats: 20 }),
    ]),
    { enabled: true, price: "20", mixed: false },
  );

  assert.deepEqual(
    derivePersonaRuntimePricing([
      instance({ pubkey: "a", pricePerMinuteSats: 20 }),
      instance({ pubkey: "b", pricePerMinuteSats: 30 }),
    ]),
    { enabled: true, price: "20", mixed: true },
  );

  // One priced instance among unpriced ones is mixed too: saving levels them.
  assert.deepEqual(
    derivePersonaRuntimePricing([
      instance({ pubkey: "a", pricePerMinuteSats: 20 }),
      instance({ pubkey: "b" }),
    ]),
    { enabled: true, price: "20", mixed: true },
  );
});

test("instances that already answer outsiders can be priced under any draft", () => {
  const external = instance({ pubkey: "a", respondTo: "allowlist" });
  const ownerOnly = instance({ pubkey: "b" });

  // The definition default is a default for future instances, not a verdict
  // on the ones already running.
  assert.equal(pricingAppliesToInstances([external], "owner-only"), true);
  assert.equal(pricingAppliesToInstances([ownerOnly], "owner-only"), false);
  assert.equal(pricingAppliesToInstances([ownerOnly], "anyone"), true);
  assert.equal(pricingAppliesToInstances([], "anyone"), false);
  // One owner-only instance among external ones still needs the draft's lift.
  assert.equal(
    pricingAppliesToInstances([external, ownerOnly], "owner-only"),
    false,
  );

  assert.deepEqual(instancesNeedingAccessLift([external, ownerOnly]), [
    ownerOnly,
  ]);
});

test("an already-external instance keeps its own access", () => {
  const updates = personaRuntimePricingUpdates({
    instances: [
      instance({
        pubkey: "a",
        respondTo: "allowlist",
        respondToAllowlist: [ALICE],
      }),
    ],
    enabled: true,
    price: "21",
    respondTo: "owner-only",
    respondToAllowlist: [],
  });

  assert.deepEqual(updates, [{ pubkey: "a", pricePerMinuteSats: 21 }]);
});

test("enabling a rate carries the access the rate needs", () => {
  const updates = personaRuntimePricingUpdates({
    instances: [instance({ pubkey: "a" })],
    enabled: true,
    price: "21",
    respondTo: "anyone",
    respondToAllowlist: [],
  });

  assert.deepEqual(updates, [
    { pubkey: "a", pricePerMinuteSats: 21, respondTo: "anyone" },
  ]);
});

test("allowlist mode carries the allowlist with the rate", () => {
  const updates = personaRuntimePricingUpdates({
    instances: [instance({ pubkey: "a" })],
    enabled: true,
    price: "21",
    respondTo: "allowlist",
    respondToAllowlist: [ALICE, BOB],
  });

  assert.deepEqual(updates, [
    {
      pubkey: "a",
      pricePerMinuteSats: 21,
      respondTo: "allowlist",
      respondToAllowlist: [ALICE, BOB],
    },
  ]);
});

test("an instance already in the requested state is not written", () => {
  const updates = personaRuntimePricingUpdates({
    instances: [
      instance({ pubkey: "a", respondTo: "anyone", pricePerMinuteSats: 21 }),
      instance({ pubkey: "b", respondTo: "anyone", pricePerMinuteSats: 15 }),
    ],
    enabled: true,
    price: "21",
    respondTo: "anyone",
    respondToAllowlist: [],
  });

  assert.deepEqual(updates, [{ pubkey: "b", pricePerMinuteSats: 21 }]);
});

test("repricing an external instance never rewrites its allowlist", () => {
  const updates = personaRuntimePricingUpdates({
    instances: [
      instance({
        pubkey: "a",
        respondTo: "allowlist",
        respondToAllowlist: [ALICE],
        pricePerMinuteSats: 15,
      }),
    ],
    enabled: true,
    price: "21",
    respondTo: "allowlist",
    respondToAllowlist: [BOB],
  });

  assert.deepEqual(updates, [{ pubkey: "a", pricePerMinuteSats: 21 }]);
});

test("lifting to allowlist without a pubkey writes nothing", () => {
  const updates = personaRuntimePricingUpdates({
    instances: [instance({ pubkey: "a" })],
    enabled: true,
    price: "21",
    respondTo: "allowlist",
    respondToAllowlist: [],
  });

  assert.deepEqual(updates, []);
});

test("clearing the rate touches only priced instances and leaves access alone", () => {
  const updates = personaRuntimePricingUpdates({
    instances: [
      instance({ pubkey: "a", respondTo: "anyone", pricePerMinuteSats: 21 }),
      instance({ pubkey: "b", respondTo: "anyone" }),
    ],
    enabled: false,
    price: "21",
    respondTo: "owner-only",
    respondToAllowlist: [],
  });

  assert.deepEqual(updates, [{ pubkey: "a", pricePerMinuteSats: null }]);
});

test("a rate the backend would reject produces no writes", () => {
  for (const [respondTo, price] of [
    ["owner-only", "21"],
    ["anyone", "0"],
    ["anyone", "-5"],
    ["anyone", "1.5"],
    ["anyone", ""],
    ["anyone", "not a number"],
  ]) {
    assert.deepEqual(
      personaRuntimePricingUpdates({
        instances: [instance({ pubkey: "a" })],
        enabled: true,
        price,
        respondTo,
        respondToAllowlist: [],
      }),
      [],
      `${respondTo} at "${price}" must not write`,
    );
  }
});

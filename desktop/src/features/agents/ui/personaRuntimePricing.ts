/**
 * Paid-runtime pricing for a definition's live instances.
 *
 * The per-runtime-minute rate is instance state, never definition state: it
 * stays out of the persona record, the community catalog, and team snapshots
 * (`AGENTS.md` rule 12). The definition editor is only an entry point — it
 * reads the rate from the live instances it would write to, and writes back
 * through `update_managed_agent`, one call per instance.
 *
 * These helpers are pure so the read/write projection is testable without a
 * dialog.
 */
import type {
  ManagedAgent,
  RespondToMode,
  UpdateManagedAgentInput,
} from "@/shared/api/types";

/** Access modes whose instances may carry a rate (`validate_runtime_price`). */
export function accessTakesPayment(mode: RespondToMode): boolean {
  return mode === "allowlist" || mode === "anyone";
}

/** Live instances minted from one definition, in listing order. */
export function personaLiveInstances(
  agents: readonly ManagedAgent[] | undefined,
  personaId: string | undefined,
): ManagedAgent[] {
  if (!personaId) return [];
  return (agents ?? []).filter((agent) => agent.personaId === personaId);
}

export type PersonaRuntimePricingState = {
  enabled: boolean;
  /** Empty when no instance carries a rate. */
  price: string;
  /** True when the instances do not all carry the same rate. */
  mixed: boolean;
};

/**
 * Project the instances' stored rates into one control state.
 *
 * The displayed rate is the first priced instance's, so a mixed set shows a
 * real rate rather than a blank field; `mixed` tells the caller to say that
 * saving levels the others to it.
 */
export function derivePersonaRuntimePricing(
  instances: readonly ManagedAgent[],
): PersonaRuntimePricingState {
  const priced = instances
    .map((instance) => instance.pricePerMinuteSats)
    .filter((rate): rate is number => rate != null && rate > 0);
  if (priced.length === 0) {
    return { enabled: false, price: "", mixed: false };
  }
  return {
    enabled: true,
    price: priced[0].toString(),
    mixed: priced.length !== instances.length || new Set(priced).size > 1,
  };
}

/**
 * Whether a rate can be applied to these instances under the draft access.
 *
 * The definition's access field is a default for future instances, so it is
 * not the whole answer: instances that already answer an external audience
 * can be priced whatever the definition default says. An owner-only instance
 * can only be priced by lifting it to the draft access, which is why an
 * owner-only draft over an owner-only instance is unsupported.
 */
export function pricingAppliesToInstances(
  instances: readonly ManagedAgent[],
  draftRespondTo: RespondToMode,
): boolean {
  if (instances.length === 0) return false;
  if (accessTakesPayment(draftRespondTo)) return true;
  return instances.every((instance) => accessTakesPayment(instance.respondTo));
}

/** Instances that a rate can only reach by opening their access. */
export function instancesNeedingAccessLift(
  instances: readonly ManagedAgent[],
): ManagedAgent[] {
  return instances.filter(
    (instance) => !accessTakesPayment(instance.respondTo),
  );
}

/**
 * The `update_managed_agent` calls that carry one pricing decision to the
 * live instances. Instances already in the requested state are skipped, so a
 * save that did not touch pricing issues no writes.
 *
 * An instance that already answers an external audience keeps its own access;
 * only an owner-only instance is lifted to the draft access, because the
 * backend rejects a rate it could never charge.
 */
export function personaRuntimePricingUpdates({
  instances,
  enabled,
  price,
  respondTo,
  respondToAllowlist,
}: {
  instances: readonly ManagedAgent[];
  enabled: boolean;
  price: string;
  respondTo: RespondToMode;
  respondToAllowlist: readonly string[];
}): UpdateManagedAgentInput[] {
  if (!enabled) {
    return instances
      .filter((instance) => instance.pricePerMinuteSats != null)
      .map((instance) => ({
        pubkey: instance.pubkey,
        pricePerMinuteSats: null,
      }));
  }

  const rate = Number(price);
  if (!Number.isSafeInteger(rate) || rate <= 0) return [];

  const allowlist = [...respondToAllowlist];
  const updates: UpdateManagedAgentInput[] = [];
  for (const instance of instances) {
    // The instance's own access wins when it can already be charged; the
    // draft is the lift for an owner-only one.
    const liftAccess = !accessTakesPayment(instance.respondTo);
    if (liftAccess && !accessTakesPayment(respondTo)) continue;
    if (liftAccess && respondTo === "allowlist" && allowlist.length === 0)
      continue;
    if (!liftAccess && instance.pricePerMinuteSats === rate) continue;
    updates.push({
      pubkey: instance.pubkey,
      pricePerMinuteSats: rate,
      ...(liftAccess
        ? {
            respondTo,
            ...(respondTo === "allowlist"
              ? { respondToAllowlist: allowlist }
              : {}),
          }
        : {}),
    });
  }
  return updates;
}

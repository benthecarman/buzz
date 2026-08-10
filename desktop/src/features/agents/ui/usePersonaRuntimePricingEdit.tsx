import * as React from "react";

import {
  useManagedAgentsQuery,
  useUpdateManagedAgentMutation,
} from "@/features/agents/hooks";
import type {
  CreatePersonaInput,
  RespondToMode,
  UpdatePersonaInput,
} from "@/shared/api/types";
import { useFeatureEnabled } from "@/shared/features/useFeatureEnabled";
import { useAgentAccessOwnerOnlyQuery } from "../useAgentAccessOwnerOnly";
import {
  MAX_RUNTIME_PRICE_PER_MINUTE_SATS,
  PaidRuntimeField,
} from "./PaidRuntimeField";
import {
  derivePersonaRuntimePricing,
  instancesNeedingAccessLift,
  personaLiveInstances,
  personaRuntimePricingUpdates,
  pricingAppliesToInstances,
} from "./personaRuntimePricing";

/**
 * Paid-runtime control for the definition editor.
 *
 * The rate is instance state (see `personaRuntimePricing.ts`): this hook reads
 * it from the definition's live instances and writes it back to them on save.
 * Nothing about the rate reaches the persona record, so a shared or exported
 * definition never carries someone else's price.
 */
export function usePersonaRuntimePricingEdit({
  initialValues,
  respondTo,
  respondToAllowlist,
  disabled,
  open,
}: {
  /** The dialog's values; only an edit (`id` present) has instances to price. */
  initialValues: CreatePersonaInput | UpdatePersonaInput | null;
  respondTo: RespondToMode;
  respondToAllowlist: readonly string[];
  disabled: boolean;
  open: boolean;
}) {
  // Charging needs the wallet that mints the agent's payment offer, so the
  // control belongs to builds where that wallet exists at all.
  const walletEnabled = useFeatureEnabled("bitcoin");
  const personaId =
    initialValues && "id" in initialValues ? initialValues.id : undefined;
  const enabledQuery = open && personaId !== undefined;
  const { data: agents } = useManagedAgentsQuery({ enabled: enabledQuery });
  const { data: ownerOnlyBuild } = useAgentAccessOwnerOnlyQuery({
    enabled: enabledQuery,
  });
  const updateMutation = useUpdateManagedAgentMutation();
  const instances = React.useMemo(
    () => personaLiveInstances(agents, personaId),
    [agents, personaId],
  );
  const stored = React.useMemo(
    () => derivePersonaRuntimePricing(instances),
    [instances],
  );

  const [enabled, setEnabled] = React.useState(stored.enabled);
  const [price, setPrice] = React.useState(stored.price);
  const [error, setError] = React.useState<string | null>(null);
  const touchedRef = React.useRef(false);
  // Instances are polled while an agent runs, so re-sync from the store only
  // until the owner takes over the control — a refetch must never overwrite
  // what they are typing.
  React.useEffect(() => {
    if (touchedRef.current) return;
    setEnabled(stored.enabled);
    setPrice(stored.price);
  }, [stored.enabled, stored.price]);

  const supported = pricingAppliesToInstances(instances, respondTo);
  const numericPrice = Number(price);
  const valid =
    !enabled ||
    !supported ||
    (Number.isSafeInteger(numericPrice) &&
      numericPrice > 0 &&
      numericPrice <= MAX_RUNTIME_PRICE_PER_MINUTE_SATS);

  /**
   * Carry the pricing decision to the live instances, reporting failure in
   * place. Runs before the definition save, which closes the dialog: a
   * rejected write has to stay readable, so `false` aborts that save.
   */
  async function apply(): Promise<boolean> {
    if (personaId === undefined || ownerOnlyBuild === true || !walletEnabled)
      return true;
    // A control the owner cannot operate must not decide anything: an
    // unsupported access selection leaves every stored rate as it is, rather
    // than reading the disabled checkbox as "clear the price".
    if (!supported) return true;
    setError(null);
    const updates = personaRuntimePricingUpdates({
      instances,
      enabled,
      price,
      respondTo,
      respondToAllowlist,
    });
    try {
      for (const update of updates) {
        await updateMutation.mutateAsync(update);
      }
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : String(cause));
      return false;
    }
    return true;
  }

  const field =
    personaId !== undefined && ownerOnlyBuild !== true && walletEnabled ? (
      <div className="space-y-1.5">
        <PaidRuntimeField
          disabled={disabled}
          enabled={enabled}
          onEnabledChange={(next) => {
            touchedRef.current = true;
            setEnabled(next);
          }}
          onPriceChange={(next) => {
            touchedRef.current = true;
            setPrice(next);
          }}
          price={price}
          supported={supported}
        />
        <p className="text-xs text-muted-foreground">
          {describeInstanceScope({
            instanceCount: instances.length,
            liftCount: supported
              ? instancesNeedingAccessLift(instances).length
              : 0,
            mixed: stored.mixed,
          })}
        </p>
        {error ? (
          <p className="text-xs text-destructive">
            Runtime pricing was not applied: {error}
          </p>
        ) : null}
      </div>
    ) : null;

  return { apply, field, valid };
}

function describeInstanceScope({
  instanceCount,
  liftCount,
  mixed,
}: {
  instanceCount: number;
  liftCount: number;
  mixed: boolean;
}): string {
  if (instanceCount === 0) {
    return "Start this agent in a community before setting a rate — the rate belongs to a running agent, not to its definition.";
  }
  const sentences = [
    instanceCount === 1
      ? "Applies to this agent's 1 running instance."
      : `Applies to this agent's ${instanceCount} running instances.`,
  ];
  if (liftCount > 0) {
    sentences.push(
      liftCount === 1
        ? "Saving also opens that instance to the access selected above."
        : `Saving also opens ${liftCount} of them to the access selected above.`,
    );
  }
  if (mixed) {
    sentences.push(
      "Their rates currently differ; saving levels them to this one.",
    );
  }
  return sentences.join(" ");
}

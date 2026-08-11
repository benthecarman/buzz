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
import { PaidRuntimeField, validatedInvocationPrice } from "./PaidRuntimeField";
import {
  derivePersonaRuntimePricing,
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
    !enabled || !supported || validatedInvocationPrice(numericPrice) !== null;

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
        {error ? (
          <p className="text-xs text-destructive">
            Reservation price was not applied: {error}
          </p>
        ) : null}
      </div>
    ) : null;

  return { apply, field, valid };
}

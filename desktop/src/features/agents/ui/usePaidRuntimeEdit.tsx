import * as React from "react";
import type { ManagedAgent, RespondToMode } from "@/shared/api/types";
import { useFeatureEnabled } from "@/shared/features/useFeatureEnabled";
import {
  DEFAULT_INVOCATION_PRICE_SATS,
  PaidRuntimeField,
  runtimePriceToAccessPrice,
  validatedInvocationPrice,
} from "./PaidRuntimeField";

export function usePaidRuntimeEdit(
  agent: ManagedAgent,
  respondTo: RespondToMode,
  ownerOnly: boolean | null | undefined,
  disabled: boolean,
) {
  // Charging needs the wallet that mints the agent's payment offer, so the
  // control belongs to builds where that wallet exists at all.
  const walletEnabled = useFeatureEnabled("bitcoin");
  const [enabled, setEnabled] = React.useState(
    agent.pricePerMinuteSats != null,
  );
  const [price, setPrice] = React.useState(
    (agent.pricePerMinuteSats == null
      ? null
      : runtimePriceToAccessPrice(agent.pricePerMinuteSats).toString()) ??
      DEFAULT_INVOCATION_PRICE_SATS.toString(),
  );
  React.useEffect(() => {
    setEnabled(agent.pricePerMinuteSats != null);
    setPrice(
      (agent.pricePerMinuteSats == null
        ? null
        : runtimePriceToAccessPrice(agent.pricePerMinuteSats).toString()) ??
        DEFAULT_INVOCATION_PRICE_SATS.toString(),
    );
  }, [agent.pricePerMinuteSats]);

  const supportsPaidRuntime =
    respondTo === "allowlist" || respondTo === "anyone";
  const numericPrice = Number(price);
  const ratePerMinute = validatedInvocationPrice(numericPrice);
  const valid =
    !walletEnabled ||
    !enabled ||
    (supportsPaidRuntime && ratePerMinute !== null);
  const update = !walletEnabled
    ? undefined
    : !supportsPaidRuntime
      ? agent.pricePerMinuteSats == null
        ? undefined
        : null
      : enabled
        ? ratePerMinute !== agent.pricePerMinuteSats
          ? ratePerMinute
          : undefined
        : agent.pricePerMinuteSats == null
          ? undefined
          : null;
  // The control stays mounted for every access mode so the capability is
  // discoverable from the dialog; owner-only access disables it with a reason
  // rather than hiding it. Only the owner-only build capability removes it.
  const field =
    ownerOnly !== true && walletEnabled ? (
      <PaidRuntimeField
        disabled={disabled}
        enabled={enabled}
        onEnabledChange={setEnabled}
        onPriceChange={setPrice}
        price={price}
        supported={supportsPaidRuntime}
      />
    ) : null;
  return { field, update, valid };
}

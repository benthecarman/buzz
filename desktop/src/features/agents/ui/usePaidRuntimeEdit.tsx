import * as React from "react";
import type { ManagedAgent, RespondToMode } from "@/shared/api/types";
import {
  MAX_RUNTIME_PRICE_PER_MINUTE_SATS,
  PaidRuntimeField,
} from "./PaidRuntimeField";

export function usePaidRuntimeEdit(
  agent: ManagedAgent,
  respondTo: RespondToMode,
  ownerOnly: boolean | null | undefined,
  disabled: boolean,
) {
  const [enabled, setEnabled] = React.useState(
    agent.pricePerMinuteSats != null,
  );
  const [price, setPrice] = React.useState(
    agent.pricePerMinuteSats?.toString() ?? "",
  );
  React.useEffect(() => {
    setEnabled(agent.pricePerMinuteSats != null);
    setPrice(agent.pricePerMinuteSats?.toString() ?? "");
  }, [agent.pricePerMinuteSats]);

  const supportsPaidRuntime =
    respondTo === "allowlist" || respondTo === "anyone";
  const numericPrice = Number(price);
  const valid =
    !enabled ||
    (supportsPaidRuntime &&
      Number.isSafeInteger(numericPrice) &&
      numericPrice > 0 &&
      numericPrice <= MAX_RUNTIME_PRICE_PER_MINUTE_SATS);
  const update = !supportsPaidRuntime
    ? agent.pricePerMinuteSats == null
      ? undefined
      : null
    : enabled
      ? numericPrice !== agent.pricePerMinuteSats
        ? numericPrice
        : undefined
      : agent.pricePerMinuteSats == null
        ? undefined
        : null;
  const field =
    supportsPaidRuntime && ownerOnly !== true ? (
      <PaidRuntimeField
        disabled={disabled}
        enabled={enabled}
        onEnabledChange={setEnabled}
        onPriceChange={setPrice}
        price={price}
      />
    ) : null;
  return { field, update, valid };
}

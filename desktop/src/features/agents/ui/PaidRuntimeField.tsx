import { Input } from "@/shared/ui/input";
import { formatSatsAsUsd } from "@/features/wallet/lib/formatBitcoin";

type PaidRuntimeFieldProps = {
  enabled: boolean;
  price: string;
  disabled?: boolean;
  /**
   * False when the selected access mode cannot take payment (owner-only). The
   * control still renders, disabled with the reason, so the capability stays
   * discoverable instead of vanishing from the dialog.
   */
  supported?: boolean;
  onEnabledChange: (enabled: boolean) => void;
  onPriceChange: (price: string) => void;
};

export const INVOCATION_WINDOW_MINUTES = 5;
export const DEFAULT_INVOCATION_PRICE_SATS = 255;
export const MAX_INVOCATION_PRICE_SATS = 150_119_987_579_016;

export function runtimePriceToAccessPrice(priceSats: number): number {
  return priceSats;
}

export function validatedInvocationPrice(priceSats: number): number | null {
  if (
    !Number.isSafeInteger(priceSats) ||
    priceSats <= 0 ||
    priceSats > MAX_INVOCATION_PRICE_SATS
  ) {
    return null;
  }
  return priceSats;
}

export const PAID_RUNTIME_UNSUPPORTED_ACCESS_REASON =
  "Set Who can send instructions to Anyone or Selected people to let others pay to invoke this agent.";

/** Instance-only pricing control for externally accessible managed agents. */
export function PaidRuntimeField({
  enabled,
  price,
  disabled = false,
  supported = true,
  onEnabledChange,
  onPriceChange,
}: PaidRuntimeFieldProps) {
  const active = supported && enabled;
  const usdPrice = /^[0-9]+$/u.test(price)
    ? formatSatsAsUsd(Number(price))
    : null;
  return (
    <div
      className="space-y-2 rounded-lg border border-border/70 bg-muted/20 p-3"
      data-testid="agent-paid-runtime"
    >
      <label className="flex items-center gap-2 text-sm font-medium text-foreground">
        <input
          checked={active}
          disabled={disabled || !supported}
          onChange={(event) => onEnabledChange(event.target.checked)}
          type="checkbox"
        />
        Require payment for agent access
      </label>
      {active ? (
        <div className="space-y-1.5">
          <label
            className="text-xs font-medium text-muted-foreground"
            htmlFor="agent-runtime-price"
          >
            Price for 5 minutes of access:
          </label>
          <div className="flex items-center gap-2">
            <Input
              disabled={disabled}
              id="agent-runtime-price"
              inputMode="numeric"
              min={1}
              max={MAX_INVOCATION_PRICE_SATS}
              onChange={(event) => onPriceChange(event.target.value)}
              pattern="[0-9]*"
              step={1}
              type="number"
              value={price}
            />
            <span className="text-sm text-muted-foreground">sats</span>
          </div>
          {usdPrice ? (
            <p className="text-xs text-muted-foreground">{usdPrice}</p>
          ) : null}
        </div>
      ) : null}
      {supported ? null : (
        <p className="text-xs text-muted-foreground">
          {PAID_RUNTIME_UNSUPPORTED_ACCESS_REASON}
        </p>
      )}
    </div>
  );
}

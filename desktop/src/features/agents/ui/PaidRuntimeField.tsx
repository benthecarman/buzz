import { Input } from "@/shared/ui/input";

type PaidRuntimeFieldProps = {
  enabled: boolean;
  price: string;
  disabled?: boolean;
  onEnabledChange: (enabled: boolean) => void;
  onPriceChange: (price: string) => void;
};

export const MAX_RUNTIME_PRICE_PER_MINUTE_SATS = 150_119_987_579_016;

/** Instance-only pricing control for externally accessible managed agents. */
export function PaidRuntimeField({
  enabled,
  price,
  disabled = false,
  onEnabledChange,
  onPriceChange,
}: PaidRuntimeFieldProps) {
  return (
    <div className="space-y-2 rounded-lg border border-border/70 bg-muted/20 p-3">
      <label className="flex items-center gap-2 text-sm font-medium text-foreground">
        <input
          checked={enabled}
          disabled={disabled}
          onChange={(event) => onEnabledChange(event.target.checked)}
          type="checkbox"
        />
        Require payment for runtime
      </label>
      {enabled ? (
        <div className="space-y-1.5">
          <label
            className="text-xs font-medium text-muted-foreground"
            htmlFor="agent-runtime-price"
          >
            ₿ per runtime minute
          </label>
          <Input
            disabled={disabled}
            id="agent-runtime-price"
            inputMode="numeric"
            min={1}
            max={MAX_RUNTIME_PRICE_PER_MINUTE_SATS}
            onChange={(event) => onPriceChange(event.target.value)}
            pattern="[0-9]*"
            step={1}
            type="number"
            value={price}
          />
        </div>
      ) : null}
      <p className="text-xs text-muted-foreground">
        People who use this agent prepay 15, 30, or 60 minutes. Only active
        agent runtime is deducted, and unused runtime remains available.
      </p>
      <p className="text-xs text-muted-foreground">
        You and your agents remain free. Paid invocation is not available in
        direct messages.
      </p>
    </div>
  );
}

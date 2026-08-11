import {
  AlertDialog,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
} from "@/shared/ui/alert-dialog";
import { Button } from "@/shared/ui/button";
import {
  agentRuntimePackChargeSats,
  agentRuntimePackRequired,
  type AgentRuntimeCapMinutes,
} from "@/features/agents/runtimePayments";

export type RuntimeCheckoutRow = {
  pubkey: string;
  name: string;
  rateSats: number;
  availableMs: number;
};

type AgentRuntimeCheckoutDialogProps = {
  capMinutes: AgentRuntimeCapMinutes;
  capLocked: boolean;
  error: string | null;
  isPaying: boolean;
  onCapChange: (cap: AgentRuntimeCapMinutes) => void;
  onConfirm: () => void;
  onDismiss: () => void;
  open: boolean;
  rows: RuntimeCheckoutRow[];
};

export function AgentRuntimeCheckoutDialog({
  capMinutes,
  capLocked,
  error,
  isPaying,
  onCapChange,
  onConfirm,
  onDismiss,
  open,
  rows,
}: AgentRuntimeCheckoutDialogProps) {
  const total = rows.reduce(
    (sum, row) =>
      sum +
      agentRuntimePackChargeSats(row.availableMs, capMinutes, row.rateSats),
    0,
  );
  return (
    <AlertDialog
      onOpenChange={(nextOpen) => {
        if (!nextOpen && !isPaying) onDismiss();
      }}
      open={open}
    >
      <AlertDialogContent data-testid="agent-runtime-checkout">
        <AlertDialogHeader>
          <AlertDialogTitle>Reserve Agent runtime</AlertDialogTitle>
          <AlertDialogDescription>
            Choose one runtime pack for each paid Agent. Retained runtime is
            used first; if it is insufficient, Buzz buys a matching pack with a
            separate BOLT12 zap at the Agent's published rate, then waits for
            the Agent to confirm the credit — usually under a minute. Unused
            runtime stays available.
          </AlertDialogDescription>
        </AlertDialogHeader>

        <fieldset className="flex gap-2">
          <legend className="sr-only">Runtime cap</legend>
          {([15, 30, 60] as const).map((cap) => (
            <Button
              key={cap}
              disabled={isPaying || capLocked}
              onClick={() => onCapChange(cap)}
              size="sm"
              type="button"
              variant={capMinutes === cap ? "default" : "outline"}
            >
              {cap} min
            </Button>
          ))}
        </fieldset>

        <div className="space-y-2">
          {rows.map((row) => {
            const needsPack = agentRuntimePackRequired(
              row.availableMs,
              capMinutes,
            );
            const retainedMinutes = row.availableMs / 60_000;
            return (
              <div
                className="rounded-lg border border-border px-3 py-2 text-sm"
                key={row.pubkey}
              >
                <div className="flex items-center justify-between gap-3">
                  <span className="font-medium">{row.name}</span>
                  <span>₿{row.rateSats}/runtime min</span>
                </div>
                <div className="mt-1 text-xs text-muted-foreground">
                  {retainedMinutes.toFixed(1)} min retained · {capMinutes} min
                  cap · {needsPack ? `₿${row.rateSats * capMinutes}` : "no zap"}
                </div>
              </div>
            );
          })}
        </div>

        <p className="text-sm">
          Combined total: <span className="font-semibold">₿{total}</span>
        </p>
        {error ? (
          <p className="rounded-lg bg-destructive/10 px-3 py-2 text-sm text-destructive">
            {error}
          </p>
        ) : null}
        <AlertDialogFooter>
          <Button
            disabled={isPaying}
            onClick={onDismiss}
            size="sm"
            type="button"
            variant="outline"
          >
            Cancel
          </Button>
          <Button
            disabled={isPaying}
            onClick={onConfirm}
            size="sm"
            type="button"
          >
            {isPaying ? "Reserving…" : total > 0 ? `Pay ₿${total}` : "Reserve"}
          </Button>
        </AlertDialogFooter>
      </AlertDialogContent>
    </AlertDialog>
  );
}

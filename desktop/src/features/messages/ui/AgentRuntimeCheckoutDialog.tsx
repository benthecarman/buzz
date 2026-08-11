import * as React from "react";
import {
  AlertDialog,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
} from "@/shared/ui/alert-dialog";
import { Button } from "@/shared/ui/button";
import { useUsersBatchQuery } from "@/features/profile/hooks";
import { resolveUserLabel } from "@/features/profile/lib/identity";
import { useIdentityQuery } from "@/shared/api/hooks";
import {
  formatBitcoin,
  formatSatsAsUsd,
} from "@/features/wallet/lib/formatBitcoin";
export type RuntimeCheckoutRow = {
  pubkey: string;
  name: string;
  ownerPubkey: string | null;
  priceSats: number;
  invocationWindowSeconds: number;
  pricingEventJson: string | null;
  needsPayment: boolean;
};

type AgentRuntimeCheckoutDialogProps = {
  error: string | null;
  isPaying: boolean;
  onConfirm: () => void;
  onDismiss: () => void;
  open: boolean;
  rows: RuntimeCheckoutRow[];
};

export function AgentRuntimeCheckoutDialog({
  error,
  isPaying,
  onConfirm,
  onDismiss,
  open,
  rows,
}: AgentRuntimeCheckoutDialogProps) {
  const confirmButtonRef = React.useRef<HTMLButtonElement>(null);
  const identityQuery = useIdentityQuery();
  const ownerPubkeys = rows.flatMap((row) =>
    row.ownerPubkey ? [row.ownerPubkey] : [],
  );
  const ownersQuery = useUsersBatchQuery(ownerPubkeys, { enabled: open });
  const total = rows.reduce(
    (sum, row) => sum + (row.needsPayment ? row.priceSats : 0),
    0,
  );
  return (
    <AlertDialog
      onOpenChange={(nextOpen) => {
        if (!nextOpen && !isPaying) onDismiss();
      }}
      open={open}
    >
      <AlertDialogContent
        data-testid="agent-runtime-checkout"
        onOpenAutoFocus={(event) => {
          event.preventDefault();
          confirmButtonRef.current?.focus();
        }}
      >
        <AlertDialogHeader>
          <AlertDialogTitle>Payment required for Agent access</AlertDialogTitle>
        </AlertDialogHeader>

        <div className="space-y-2">
          {rows
            .filter((row) => row.needsPayment)
            .map((row) => {
              const usdPrice = formatSatsAsUsd(row.priceSats);
              const ownerName = row.ownerPubkey
                ? resolveUserLabel({
                    pubkey: row.ownerPubkey,
                    currentPubkey: identityQuery.data?.pubkey,
                    profiles: ownersQuery.data?.profiles,
                  })
                : "this user";
              const ownerPossessive =
                ownerName === "You" ? "your" : `${ownerName}'s`;
              return (
                <AlertDialogDescription key={row.pubkey}>
                  Using {ownerPossessive} agent {row.name} costs{" "}
                  {formatBitcoin(row.priceSats)}
                  {usdPrice ? ` (${usdPrice})` : ""} for 5 minutes of access.
                </AlertDialogDescription>
              );
            })}
        </div>
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
            ref={confirmButtonRef}
            size="sm"
            type="button"
          >
            {isPaying
              ? "Paying…"
              : total > 0
                ? `Pay ${formatBitcoin(total)}`
                : "Continue"}
          </Button>
        </AlertDialogFooter>
      </AlertDialogContent>
    </AlertDialog>
  );
}

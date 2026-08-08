import { useEffect, useState, type FormEvent } from "react";
import { useQueryClient } from "@tanstack/react-query";
import { Bitcoin, LoaderCircle, TriangleAlert } from "lucide-react";
import { toast } from "sonner";

import {
  getPendingProfileZap,
  getRecipientWalletOffer,
  sendProfileZap,
} from "../api";
import { formatBitcoin } from "../lib/formatBitcoin";
import { placeholderMessageZapsQueryKey } from "../lib/placeholderMessageZaps";
import { parseWholeBitcoinAmount } from "../lib/profileZap";
import { walletCommandError } from "../lib/walletError";
import { Button } from "@/shared/ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/shared/ui/dialog";
import { Input } from "@/shared/ui/input";

export function SendBitcoinDialog({
  onOpenChange,
  open,
  recipientName,
  recipientPubkey,
  targetEventId = null,
  targetEventKind = null,
}: {
  onOpenChange: (open: boolean) => void;
  open: boolean;
  recipientName: string;
  recipientPubkey: string;
  targetEventId?: string | null;
  targetEventKind?: number | null;
}) {
  const queryClient = useQueryClient();
  const [amount, setAmount] = useState("");
  const [comment, setComment] = useState("");
  const [idempotencyKey, setIdempotencyKey] = useState<string>(
    crypto.randomUUID(),
  );
  const [offerState, setOfferState] = useState<
    "idle" | "loading" | "ready" | "missing"
  >("idle");
  const [offerError, setOfferError] = useState<string | null>(null);
  const [pendingState, setPendingState] = useState<
    "loading" | "ready" | "failed"
  >("loading");
  const [sending, setSending] = useState(false);
  const [reconciling, setReconciling] = useState(false);

  useEffect(() => {
    if (!open) return;
    let cancelled = false;
    setOfferState("loading");
    setOfferError(null);
    setPendingState("loading");
    getRecipientWalletOffer(recipientPubkey)
      .then(() => {
        if (!cancelled) setOfferState("ready");
      })
      .catch((error) => {
        if (cancelled) return;
        const commandError = walletCommandError(error);
        setOfferState("missing");
        setOfferError(
          commandError.message ??
            `${recipientName} has not enabled their Bitcoin wallet.`,
        );
      });
    getPendingProfileZap(recipientPubkey, targetEventId)
      .then((pending) => {
        if (cancelled) return;
        if (pending) {
          setAmount(String(pending.amount));
          setComment(pending.comment ?? "");
          setIdempotencyKey(pending.idempotencyKey);
          setReconciling(true);
        } else {
          setAmount("");
          setComment("");
          setIdempotencyKey(crypto.randomUUID());
          setReconciling(false);
        }
        setPendingState("ready");
      })
      .catch(() => {
        // A failed pending-attempt lookup must not look like "no pending
        // attempt": minting a fresh idempotency key here could turn a
        // reconcile into a duplicate payment. Keep sending disabled.
        if (!cancelled) setPendingState("failed");
      });
    return () => {
      cancelled = true;
    };
  }, [open, recipientName, recipientPubkey, targetEventId]);

  const parsedAmount = parseWholeBitcoinAmount(amount);
  const validAmount = parsedAmount !== null;

  async function handleSubmit(event: FormEvent) {
    event.preventDefault();
    if (
      !validAmount ||
      sending ||
      offerState !== "ready" ||
      pendingState !== "ready"
    )
      return;
    setSending(true);
    try {
      const result = await sendProfileZap({
        recipientPubkey,
        amount: parsedAmount,
        comment: comment.trim() || null,
        idempotencyKey,
        targetEventId,
        targetEventKind,
      });
      if (result.payment.status === "completed") {
        if (targetEventId) {
          await queryClient.invalidateQueries({
            queryKey: placeholderMessageZapsQueryKey,
          });
        }
        toast.success(
          `${formatBitcoin(result.payment.amount ?? parsedAmount)} sent`,
          {
            description: targetEventId
              ? "The payment settled. A local placeholder receipt now appears under the message while payer proofs are unavailable."
              : "The payment settled. Buzz kept the intent local because the wallet cannot produce an lnp payer proof yet.",
          },
        );
        setReconciling(false);
        onOpenChange(false);
      } else if (result.payment.status === "failed") {
        setReconciling(false);
        setIdempotencyKey(crypto.randomUUID());
        toast.error(
          result.payment.statusMessage || "The Bitcoin payment failed.",
        );
      } else {
        setReconciling(true);
        toast.warning("The payment is still pending. Buzz will reconcile it.");
      }
    } catch (error) {
      const commandError = walletCommandError(error);
      if (commandError.code === "payment_status_unknown") {
        setReconciling(true);
      } else if (commandError.code === "payment_failed") {
        setReconciling(false);
        setIdempotencyKey(crypto.randomUUID());
      }
      toast.error(commandError.message ?? "The Bitcoin payment failed.");
    } finally {
      setSending(false);
    }
  }

  return (
    <Dialog onOpenChange={sending ? undefined : onOpenChange} open={open}>
      <DialogContent className="sm:max-w-md" data-testid="send-bitcoin-dialog">
        <DialogHeader>
          <DialogTitle className="flex items-center gap-2">
            <Bitcoin className="h-5 w-5" />
            Send bitcoin
          </DialogTitle>
          <DialogDescription>
            {targetEventId
              ? `Zap ${recipientName}'s message from your Buzz wallet.`
              : `Pay ${recipientName}'s BOLT12 offer from your Buzz wallet.`}
          </DialogDescription>
        </DialogHeader>

        {offerState === "loading" ? (
          <div className="flex items-center gap-2 rounded-lg bg-muted/40 p-4 text-sm text-muted-foreground">
            <LoaderCircle className="h-4 w-4 animate-spin" />
            Checking their wallet…
          </div>
        ) : null}

        {offerState === "missing" ? (
          <div
            className="flex items-start gap-3 rounded-lg border border-amber-500/30 bg-amber-500/10 p-4 text-sm"
            data-testid="send-bitcoin-offer-missing"
          >
            <TriangleAlert className="mt-0.5 h-4 w-4 shrink-0 text-amber-600" />
            <div>
              <p className="font-medium">Wallet not set up</p>
              <p className="mt-1 text-muted-foreground">{offerError}</p>
            </div>
          </div>
        ) : null}

        {pendingState === "failed" ? (
          <div
            className="flex items-start gap-3 rounded-lg border border-amber-500/30 bg-amber-500/10 p-4 text-sm"
            data-testid="send-bitcoin-pending-check-failed"
          >
            <TriangleAlert className="mt-0.5 h-4 w-4 shrink-0 text-amber-600" />
            <div>
              <p className="font-medium">
                Could not check for a previous payment
              </p>
              <p className="mt-1 text-muted-foreground">
                Buzz could not read its local payment history. Sending is
                disabled to avoid a duplicate payment.
              </p>
            </div>
          </div>
        ) : null}

        <form
          className="space-y-4"
          onSubmit={(event) => void handleSubmit(event)}
        >
          {reconciling ? (
            <div className="rounded-lg border border-amber-500/30 bg-amber-500/10 p-3 text-sm">
              The previous result is unknown. Retry reconciles that payment and
              will not send again.
            </div>
          ) : null}
          <label className="block space-y-1.5" htmlFor="profile-bitcoin-amount">
            <span className="text-sm font-medium">Amount in ₿</span>
            <Input
              autoFocus
              disabled={
                sending ||
                reconciling ||
                offerState !== "ready" ||
                pendingState !== "ready"
              }
              id="profile-bitcoin-amount"
              inputMode="numeric"
              min="1"
              onChange={(event) => setAmount(event.target.value)}
              placeholder="21000"
              step="1"
              type="text"
              value={amount}
            />
          </label>
          <label
            className="block space-y-1.5"
            htmlFor="profile-bitcoin-comment"
          >
            <span className="text-sm font-medium">Comment (optional)</span>
            <Input
              disabled={
                sending ||
                reconciling ||
                offerState !== "ready" ||
                pendingState !== "ready"
              }
              id="profile-bitcoin-comment"
              maxLength={280}
              onChange={(event) => setComment(event.target.value)}
              placeholder="Great work"
              value={comment}
            />
          </label>
          {validAmount ? (
            <p className="text-sm text-muted-foreground">
              You&apos;ll send {formatBitcoin(parsedAmount)}.
            </p>
          ) : null}
          <DialogFooter>
            <Button
              disabled={sending}
              onClick={() => onOpenChange(false)}
              type="button"
              variant="ghost"
            >
              Cancel
            </Button>
            <Button
              disabled={
                !validAmount ||
                sending ||
                offerState !== "ready" ||
                pendingState !== "ready"
              }
              type="submit"
            >
              {sending ? (
                <LoaderCircle className="mr-2 h-4 w-4 animate-spin" />
              ) : null}
              {reconciling ? "Check payment" : "Send bitcoin"}
            </Button>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  );
}

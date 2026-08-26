import * as React from "react";
import { toast } from "sonner";

import {
  hostedAgentZapTarget,
  type HostedAgentZapTarget,
} from "@/features/messages/lib/hostedAgentZap";
import type { TimelineMessage } from "@/features/messages/types";
import { getPendingProfileZap, sendProfileZap } from "@/features/wallet/api";
import { useBitcoinCompileEnabled } from "@/features/wallet/hooks";
import { walletCommandError } from "@/features/wallet/lib/walletError";
import { useIdentityQuery } from "@/shared/api/hooks";
import { KIND_HUDDLE_STARTED } from "@/shared/constants/kinds";
import { useFeatureEnabled } from "@/shared/features";

const MESSAGE_ZAP_AMOUNT = 50;
const MESSAGE_ZAP_RECONCILE_INITIAL_MS = 1_000;
const MESSAGE_ZAP_RECONCILE_MAX_MS = 15_000;
const activeMessageZapClaims = new Map<string, string>();

export type OptimisticZap = {
  amount: number;
  idempotencyKey: string;
  intentEventId?: string;
};

export type MessageZapAction = {
  amount: number;
  canZap: boolean;
  disabled: boolean;
  hostedTarget: HostedAgentZapTarget | null;
  label: string;
  run: () => void;
};

/** Build one shared zap action for the message card and its action bar. */
export function useMessageZap({
  channelId,
  disabled = false,
  message,
  onOptimisticZapChange,
}: {
  channelId?: string | null;
  disabled?: boolean;
  message: TimelineMessage;
  onOptimisticZapChange?: (zap: OptimisticZap | null) => void;
}): MessageZapAction {
  const attemptGenerationRef = React.useRef(0);
  const inFlightRef = React.useRef(false);
  const wasDisabledRef = React.useRef(disabled);
  const bitcoinEnabled = useFeatureEnabled("bitcoin");
  const bitcoinCompiled = useBitcoinCompileEnabled();
  const currentPubkey = useIdentityQuery().data?.pubkey;
  const hostedTarget = hostedAgentZapTarget(message, channelId);
  const amount = hostedTarget?.amount ?? MESSAGE_ZAP_AMOUNT;
  const label = hostedTarget
    ? `${hostedTarget.leaseId ? "Renew" : "Start"} agent for ₿${amount}/hour`
    : `Zap ₿${amount}`;
  const canZap =
    bitcoinEnabled &&
    bitcoinCompiled &&
    !message.pending &&
    message.kind !== KIND_HUDDLE_STARTED &&
    message.kind !== undefined &&
    Boolean(message.pubkey) &&
    message.pubkey !== currentPubkey;

  React.useEffect(() => {
    if (wasDisabledRef.current && !disabled) {
      attemptGenerationRef.current += 1;
      inFlightRef.current = false;
    }
    wasDisabledRef.current = disabled;
  }, [disabled]);

  React.useEffect(
    () => () => {
      attemptGenerationRef.current += 1;
      inFlightRef.current = false;
    },
    [],
  );

  const run = React.useCallback(() => {
    if (
      disabled ||
      inFlightRef.current ||
      !message.pubkey ||
      message.kind === undefined
    ) {
      return;
    }

    const recipientPubkey = message.pubkey;
    const targetEventKind = message.kind;
    const targetEventId = hostedTarget?.targetEventId ?? message.id;
    if (activeMessageZapClaims.has(message.id)) return;
    const idempotencyKey = crypto.randomUUID();
    activeMessageZapClaims.set(message.id, idempotencyKey);
    const optimisticZap: OptimisticZap = { amount, idempotencyKey };
    const attemptGeneration = ++attemptGenerationRef.current;
    const isCurrentAttempt = () =>
      attemptGenerationRef.current === attemptGeneration;
    inFlightRef.current = true;
    onOptimisticZapChange?.(optimisticZap);

    void (async () => {
      let submittedZap = optimisticZap;
      let reconcileDelayMs = MESSAGE_ZAP_RECONCILE_INITIAL_MS;
      let reconcilingPersistedAttempt = false;
      try {
        const pendingZap = await getPendingProfileZap(
          recipientPubkey,
          targetEventId,
        );
        if (!isCurrentAttempt()) return;
        reconcilingPersistedAttempt = Boolean(pendingZap);
        const request = pendingZap ?? {
          recipientPubkey,
          amount,
          comment: null,
          idempotencyKey,
          targetEventId,
          targetEventKind,
          channelId: hostedTarget?.channelId ?? channelId ?? null,
          leaseId: hostedTarget?.leaseId ?? null,
        };
        if (pendingZap) {
          submittedZap = {
            amount: pendingZap.amount,
            idempotencyKey: pendingZap.idempotencyKey,
          };
          onOptimisticZapChange?.(submittedZap);
        }

        while (isCurrentAttempt()) {
          try {
            const result = await sendProfileZap(request);
            if (!isCurrentAttempt()) return;
            if (result.payment.status === "failed") {
              onOptimisticZapChange?.(null);
              toast.error(
                result.payment.statusMessage || "The Bitcoin payment failed.",
              );
              return;
            }

            onOptimisticZapChange?.({
              ...submittedZap,
              intentEventId: result.intentEventId,
            });
            if (result.payment.status === "completed") return;
            reconcilingPersistedAttempt = true;
          } catch (error) {
            if (!isCurrentAttempt()) return;
            const commandError = walletCommandError(error);
            if (
              commandError.code === "payment_status_unknown" ||
              commandError.code === "relay_publish_failed"
            ) {
              reconcilingPersistedAttempt = true;
            }
            if (
              commandError.code === "payment_failed" ||
              !reconcilingPersistedAttempt
            ) {
              onOptimisticZapChange?.(null);
              toast.error(
                commandError.message ?? "The Bitcoin payment failed.",
              );
              return;
            }
          }

          await new Promise((resolve) =>
            window.setTimeout(resolve, reconcileDelayMs),
          );
          reconcileDelayMs = Math.min(
            reconcileDelayMs * 2,
            MESSAGE_ZAP_RECONCILE_MAX_MS,
          );
        }
      } catch (error) {
        if (!isCurrentAttempt()) return;
        onOptimisticZapChange?.(null);
        toast.error(
          walletCommandError(error).message ?? "The Bitcoin payment failed.",
        );
      } finally {
        if (isCurrentAttempt()) inFlightRef.current = false;
        if (activeMessageZapClaims.get(message.id) === idempotencyKey) {
          activeMessageZapClaims.delete(message.id);
        }
      }
    })();
  }, [
    amount,
    channelId,
    disabled,
    hostedTarget,
    message,
    onOptimisticZapChange,
  ]);

  return { amount, canZap, disabled, hostedTarget, label, run };
}

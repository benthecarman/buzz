import * as React from "react";
import { toast } from "sonner";

import { useManagedAgentsQuery } from "@/features/agents/hooks";
import {
  requestDockBounce,
  sendDesktopNotification,
} from "@/features/notifications/lib/desktop";
import type { NotificationSettings } from "@/features/notifications/hooks";
import { relayClient } from "@/shared/api/relayClient";
import type { RelayEvent } from "@/shared/api/types";
import { useFeatureEnabled } from "@/shared/features";
import type { WalletVerifiedZapEvent } from "./types";
import { listWalletTransactions, parseWalletZapEvents } from "./api";
import { formatBitcoin } from "./lib/formatBitcoin";
import { zapSubscriptionFilter } from "./lib/zapEvents";
import { persistZapHistoryItem } from "./lib/zapHistory";
import {
  fetchZapCatchupEvents,
  hasSettledZapPayment,
  readZapSyncCursor,
  type ZapSyncScope,
  ZAP_SYNC_OVERLAP_SECONDS,
  writeZapSyncCursor,
} from "./lib/zapNotificationSync";

const RETRY_BASE_MS = 1_000;
const RETRY_MAX_MS = 30_000;

type ZapProcessingResult =
  | { status: "processed"; recipientPubkey: string | null }
  | { status: "retry"; recipientPubkey: string | null };

/** Subscribe to settled zap proofs tagging the user or one of their agents. */
export function useZapNotifications(
  ownerPubkey: string | undefined,
  notificationSettings: NotificationSettings,
  relayUrl: string | undefined,
) {
  const walletEnabled = useFeatureEnabled("bitcoin");
  const managedAgents =
    useManagedAgentsQuery({ enabled: walletEnabled }).data ?? [];
  const recipientNames = React.useMemo(() => {
    const names = new Map<string, string>();
    const owner = ownerPubkey?.trim().toLowerCase();
    if (owner) names.set(owner, "You");
    for (const agent of managedAgents) {
      names.set(
        agent.pubkey.trim().toLowerCase(),
        agent.name.trim() || "Agent",
      );
    }
    return names;
  }, [managedAgents, ownerPubkey]);
  const recipientKey = [...recipientNames.keys()].sort().join(",");

  const handleZap = React.useEffectEvent(
    async (event: RelayEvent): Promise<ZapProcessingResult> => {
      const owner = ownerPubkey?.trim().toLowerCase();
      if (!owner) return { status: "processed", recipientPubkey: null };
      let parsedZaps: WalletVerifiedZapEvent[];
      try {
        parsedZaps = await parseWalletZapEvents(
          [event],
          [...recipientNames.keys()],
        );
      } catch (error) {
        console.error("Failed to validate tagged zap", error);
        return { status: "retry", recipientPubkey: null };
      }
      const [zap] = parsedZaps;
      if (!zap) return { status: "processed", recipientPubkey: null };

      try {
        if (
          !(await hasSettledZapPayment({
            amount: zap.amount,
            intentEventId: zap.intentEventId,
            listTransactions: listWalletTransactions,
          }))
        ) {
          return {
            // The proof commonly reaches the relay before Lexe's inbound
            // payment index catches up. Do not advance the durable relay
            // cursor until the wallet can correlate it, or the received zap
            // disappears permanently from history.
            status: "retry",
            recipientPubkey: zap.recipientPubkey,
          };
        }
      } catch (error) {
        console.error(
          "Failed to correlate tagged zap with wallet history",
          error,
        );
        return { status: "retry", recipientPubkey: zap.recipientPubkey };
      }

      const recipientName = recipientNames.get(zap.recipientPubkey) ?? "You";
      const title =
        recipientName === "You"
          ? "You received a zap"
          : `${recipientName} received a zap`;
      const body = zap.comment.trim()
        ? `${formatBitcoin(zap.amount)} · ${zap.comment.trim().slice(0, 160)}`
        : formatBitcoin(zap.amount);
      const didPersist = persistZapHistoryItem(owner, {
        amount: zap.amount,
        comment: zap.comment,
        createdAt: event.created_at,
        eventId: event.id,
        intentEventId: zap.intentEventId,
        payerPubkey: event.pubkey,
        recipientName,
        recipientPubkey: zap.recipientPubkey,
        targetEventId: zap.targetEventId,
      });
      if (!didPersist) {
        return {
          status: "processed",
          recipientPubkey: zap.recipientPubkey,
        };
      }

      toast.success(title, { description: body });
      void requestDockBounce();
      if (!notificationSettings.desktopEnabled) {
        return {
          status: "processed",
          recipientPubkey: zap.recipientPubkey,
        };
      }

      const didSend = await sendDesktopNotification({
        title,
        body,
        target: {
          channelId: null,
          content: zap.comment,
          createdAt: event.created_at,
          eventId: zap.targetEventId ?? event.id,
          kind: event.kind,
          pubkey: event.pubkey,
          threadRootId: null,
        },
      });
      if (!didSend) {
        console.error("Failed to send desktop notification for received zap");
      }
      return {
        status: "processed",
        recipientPubkey: zap.recipientPubkey,
      };
    },
  );

  React.useEffect(() => {
    const owner = ownerPubkey?.trim().toLowerCase();
    const normalizedRelayUrl = relayUrl?.trim().replace(/\/$/, "") ?? "";
    if (!walletEnabled || !owner || !normalizedRelayUrl || !recipientKey)
      return;
    let cancelled = false;
    let disposer: (() => Promise<void>) | null = null;
    let retryTimer: ReturnType<typeof globalThis.setTimeout> | null = null;
    let retryAttempt = 0;
    let generation = 0;
    const recipients = recipientKey.split(",");

    const scopeFor = (recipientPubkey: string): ZapSyncScope => ({
      ownerPubkey: owner,
      recipientPubkey,
      relayUrl: normalizedRelayUrl,
    });

    const scheduleRetry = (error: unknown) => {
      if (cancelled || retryTimer) return;
      console.error("Failed to synchronize zap events; retrying", error);
      const delay = Math.min(
        RETRY_MAX_MS,
        RETRY_BASE_MS * 2 ** Math.min(retryAttempt, 5),
      );
      retryAttempt += 1;
      retryTimer = globalThis.setTimeout(() => {
        retryTimer = null;
        void subscribe();
      }, delay);
    };

    const subscribe = async () => {
      const attempt = ++generation;
      const syncUntil = Math.floor(Date.now() / 1_000);
      const bufferedEvents = new Map<string, RelayEvent>();
      let caughtUp = false;
      let processing = Promise.resolve();

      const isCurrent = () => !cancelled && generation === attempt;
      const stopAttempt = async (error: unknown) => {
        if (!isCurrent()) return;
        generation += 1;
        caughtUp = false;
        const activeDisposer = disposer;
        disposer = null;
        try {
          await activeDisposer?.();
        } catch (disposeError) {
          console.error("Failed to close zap subscription", disposeError);
        }
        scheduleRetry(error);
      };
      const processLiveEvent = async (event: RelayEvent) => {
        const result = await handleZap(event);
        if (!isCurrent()) return;
        if (result.status === "retry") {
          bufferedEvents.set(event.id, event);
          throw new Error("Zap wallet correlation is temporarily unavailable.");
        }
        if (result.recipientPubkey) {
          writeZapSyncCursor(
            scopeFor(result.recipientPubkey),
            event.created_at,
          );
        }
      };
      const enqueueLiveEvent = (event: RelayEvent) => {
        processing = processing
          .then(() => processLiveEvent(event))
          .catch((error) => stopAttempt(error));
      };

      try {
        const nextDisposer = await relayClient.subscribeLive(
          zapSubscriptionFilter(
            recipients,
            Math.max(0, syncUntil - ZAP_SYNC_OVERLAP_SECONDS),
          ),
          (event) => {
            if (!isCurrent()) return;
            if (!caughtUp) {
              bufferedEvents.set(event.id, event);
              return;
            }
            enqueueLiveEvent(event);
          },
        );
        if (!isCurrent()) {
          await nextDisposer();
          return;
        }
        disposer = nextDisposer;

        for (const recipient of recipients) {
          const scope = scopeFor(recipient);
          const cursor = readZapSyncCursor(scope);
          const events = await fetchZapCatchupEvents({
            recipientPubkey: recipient,
            since: cursor,
            until: syncUntil,
            fetchPage: (filter) => relayClient.fetchEvents(filter),
          });
          for (const event of events) {
            if (!isCurrent()) return;
            const result = await handleZap(event);
            if (result.status === "retry") {
              throw new Error(
                "Zap wallet correlation is temporarily unavailable.",
              );
            }
            writeZapSyncCursor(scope, event.created_at);
            bufferedEvents.delete(event.id);
          }
        }

        if (!isCurrent()) return;
        caughtUp = true;
        retryAttempt = 0;
        const buffered = [...bufferedEvents.values()].sort(
          (left, right) =>
            left.created_at - right.created_at ||
            left.id.localeCompare(right.id),
        );
        bufferedEvents.clear();
        for (const event of buffered) enqueueLiveEvent(event);
      } catch (error) {
        await stopAttempt(error);
      }
    };
    void subscribe();
    return () => {
      cancelled = true;
      generation += 1;
      if (retryTimer) globalThis.clearTimeout(retryTimer);
      void disposer?.();
    };
  }, [ownerPubkey, recipientKey, relayUrl, walletEnabled]);
}

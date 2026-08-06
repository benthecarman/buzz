import * as React from "react";

import { useManagedAgentsQuery } from "@/features/agents/hooks";
import {
  requestDockBounce,
  sendDesktopNotification,
} from "@/features/notifications/lib/desktop";
import type { NotificationSettings } from "@/features/notifications/hooks";
import { relayClient } from "@/shared/api/relayClient";
import type { RelayEvent } from "@/shared/api/types";
import { listWalletTransactions } from "./api";
import { formatBitcoin } from "./lib/formatBitcoin";
import { parseTaggedZapEvent, zapSubscriptionFilter } from "./lib/zapEvents";

const RETRY_BASE_MS = 1_000;
const RETRY_MAX_MS = 30_000;
const NOTIFIED_LIMIT = 500;

function notifiedStorageKey(ownerPubkey: string) {
  return `buzz-wallet-zap-notified.v1:${ownerPubkey}`;
}

function loadNotified(ownerPubkey: string): Set<string> {
  try {
    const value = JSON.parse(
      localStorage.getItem(notifiedStorageKey(ownerPubkey)) ?? "[]",
    );
    return new Set(
      Array.isArray(value)
        ? value.filter((id): id is string => typeof id === "string")
        : [],
    );
  } catch {
    return new Set();
  }
}

function saveNotified(ownerPubkey: string, notified: Set<string>) {
  try {
    localStorage.setItem(
      notifiedStorageKey(ownerPubkey),
      JSON.stringify([...notified].slice(-NOTIFIED_LIMIT)),
    );
  } catch {
    // Notification deduplication is best effort when storage is unavailable.
  }
}

function paymentMatchesIntent(
  transaction: Awaited<
    ReturnType<typeof listWalletTransactions>
  >["transactions"][number],
  intentEventId: string,
  amount: number,
) {
  return (
    transaction.direction === "inbound" &&
    transaction.status === "completed" &&
    transaction.amount === amount &&
    transaction.payerNote === `nostr:nipB1:${intentEventId}`
  );
}

/** Subscribe to settled zap proofs tagging the user or one of their agents. */
export function useZapNotifications(
  ownerPubkey: string | undefined,
  walletEnabled: boolean,
  notificationSettings: NotificationSettings,
) {
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

  const handleZap = React.useEffectEvent(async (event: RelayEvent) => {
    const owner = ownerPubkey?.trim().toLowerCase();
    if (!owner) return;
    const notified = loadNotified(owner);
    if (notified.has(event.id)) return;
    const zap = parseTaggedZapEvent(event, new Set(recipientNames.keys()));
    if (!zap) return;

    let page: Awaited<ReturnType<typeof listWalletTransactions>>;
    try {
      page = await listWalletTransactions(undefined, true);
    } catch (error) {
      console.error(
        "Failed to correlate tagged zap with wallet history",
        error,
      );
      return;
    }
    if (
      !page.transactions.some((transaction) =>
        paymentMatchesIntent(transaction, zap.intentEventId, zap.amount),
      )
    ) {
      return;
    }
    if (!notificationSettings.desktopEnabled) return;

    const recipientName = recipientNames.get(zap.recipientPubkey) ?? "You";
    const didSend = await sendDesktopNotification({
      title:
        recipientName === "You"
          ? "You received a zap"
          : `${recipientName} received a zap`,
      body: zap.comment.trim()
        ? `${formatBitcoin(zap.amount)} · ${zap.comment.trim().slice(0, 160)}`
        : formatBitcoin(zap.amount),
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
    if (!didSend) return;
    notified.add(event.id);
    saveNotified(owner, notified);
    void requestDockBounce();
  });

  React.useEffect(() => {
    if (!walletEnabled || !recipientKey) return;
    let cancelled = false;
    let disposer: (() => Promise<void>) | null = null;
    let retryTimer: ReturnType<typeof globalThis.setTimeout> | null = null;
    let retryAttempt = 0;
    const since = Math.floor(Date.now() / 1_000) - 5;

    const subscribe = () => {
      void relayClient
        .subscribeLive(
          zapSubscriptionFilter(recipientKey.split(","), since),
          (event) => void handleZap(event),
        )
        .then((nextDisposer) => {
          if (cancelled) {
            void nextDisposer();
            return;
          }
          retryAttempt = 0;
          disposer = nextDisposer;
        })
        .catch((error) => {
          if (cancelled) return;
          console.error("Failed to subscribe to zap events; retrying", error);
          const delay = Math.min(
            RETRY_MAX_MS,
            RETRY_BASE_MS * 2 ** Math.min(retryAttempt, 5),
          );
          retryAttempt += 1;
          retryTimer = globalThis.setTimeout(subscribe, delay);
        });
    };
    subscribe();
    return () => {
      cancelled = true;
      if (retryTimer) globalThis.clearTimeout(retryTimer);
      void disposer?.();
    };
  }, [recipientKey, walletEnabled]);
}

import * as React from "react";

import type { FeedItem } from "@/shared/api/types";
import { KIND_BOLT12_ZAP } from "@/shared/constants/kinds";
import { formatBitcoin } from "./formatBitcoin";

export type ZapHistoryItem = {
  amount: number;
  comment: string;
  createdAt: number;
  eventId: string;
  intentEventId: string;
  payerPubkey: string;
  recipientName: string;
  recipientPubkey: string;
  targetEventId: string | null;
};

const HISTORY_LIMIT = 500;
const HISTORY_EVENT = "buzz:wallet-zap-history-updated";

function storageKey(ownerPubkey: string) {
  return `buzz-wallet-zap-history.v1:${ownerPubkey.trim().toLowerCase()}`;
}

function isZapHistoryItem(value: unknown): value is ZapHistoryItem {
  if (!value || typeof value !== "object") return false;
  const item = value as Partial<ZapHistoryItem>;
  return (
    typeof item.amount === "number" &&
    Number.isSafeInteger(item.amount) &&
    item.amount > 0 &&
    typeof item.comment === "string" &&
    typeof item.createdAt === "number" &&
    Number.isSafeInteger(item.createdAt) &&
    typeof item.eventId === "string" &&
    typeof item.intentEventId === "string" &&
    typeof item.payerPubkey === "string" &&
    typeof item.recipientName === "string" &&
    typeof item.recipientPubkey === "string" &&
    (item.targetEventId === null || typeof item.targetEventId === "string")
  );
}

export function parseZapHistory(raw: string | null): ZapHistoryItem[] {
  try {
    const value = JSON.parse(raw ?? "[]");
    if (!Array.isArray(value)) return [];
    return value
      .filter(isZapHistoryItem)
      .sort((left, right) => right.createdAt - left.createdAt)
      .slice(0, HISTORY_LIMIT);
  } catch {
    return [];
  }
}

export function addZapHistoryItem(
  existing: readonly ZapHistoryItem[],
  item: ZapHistoryItem,
): { didAdd: boolean; items: ZapHistoryItem[] } {
  if (existing.some((candidate) => candidate.eventId === item.eventId)) {
    return { didAdd: false, items: [...existing] };
  }
  return {
    didAdd: true,
    items: [item, ...existing]
      .sort((left, right) => right.createdAt - left.createdAt)
      .slice(0, HISTORY_LIMIT),
  };
}

export function readZapHistory(ownerPubkey: string): ZapHistoryItem[] {
  if (typeof window === "undefined" || !ownerPubkey.trim()) return [];
  try {
    return parseZapHistory(localStorage.getItem(storageKey(ownerPubkey)));
  } catch {
    return [];
  }
}

export function persistZapHistoryItem(
  ownerPubkey: string,
  item: ZapHistoryItem,
): boolean {
  if (typeof window === "undefined" || !ownerPubkey.trim()) return false;
  const result = addZapHistoryItem(readZapHistory(ownerPubkey), item);
  if (!result.didAdd) return false;
  try {
    localStorage.setItem(storageKey(ownerPubkey), JSON.stringify(result.items));
  } catch {
    return false;
  }
  window.dispatchEvent(
    new CustomEvent(HISTORY_EVENT, {
      detail: { ownerPubkey: ownerPubkey.trim().toLowerCase() },
    }),
  );
  return true;
}

export function useZapHistory(ownerPubkey: string | undefined) {
  const normalizedOwner = ownerPubkey?.trim().toLowerCase() ?? "";
  const [items, setItems] = React.useState<ZapHistoryItem[]>(() =>
    readZapHistory(normalizedOwner),
  );

  React.useEffect(() => {
    setItems(readZapHistory(normalizedOwner));
    function refresh(event: Event) {
      if (
        event instanceof CustomEvent &&
        event.detail?.ownerPubkey &&
        event.detail.ownerPubkey !== normalizedOwner
      ) {
        return;
      }
      setItems(readZapHistory(normalizedOwner));
    }
    function refreshFromStorage(event: StorageEvent) {
      if (event.key === storageKey(normalizedOwner)) refresh(event);
    }
    window.addEventListener(HISTORY_EVENT, refresh);
    window.addEventListener("storage", refreshFromStorage);
    return () => {
      window.removeEventListener(HISTORY_EVENT, refresh);
      window.removeEventListener("storage", refreshFromStorage);
    };
  }, [normalizedOwner]);

  return items;
}

export function zapHistoryFeedItems(
  items: readonly ZapHistoryItem[],
): FeedItem[] {
  return items.map((item) => ({
    id: item.eventId,
    kind: KIND_BOLT12_ZAP,
    pubkey: item.payerPubkey,
    content: item.comment.trim()
      ? `${formatBitcoin(item.amount)} · ${item.comment.trim()}`
      : `${formatBitcoin(item.amount)} received by ${item.recipientName}`,
    createdAt: item.createdAt,
    channelId: null,
    channelName: "",
    tags: [
      ["p", item.recipientPubkey],
      ["amount", String(item.amount * 1_000)],
      ["buzz_recipient_name", item.recipientName],
      ...(item.targetEventId ? [["e", item.targetEventId]] : []),
    ],
    category: "activity",
  }));
}

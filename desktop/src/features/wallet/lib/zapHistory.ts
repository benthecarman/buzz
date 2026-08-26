import * as React from "react";

export type ZapHistoryItem = {
  amount: number;
  channelId: string | null;
  comment: string;
  createdAt: number;
  eventId: string;
  intentEventId: string;
  leaseId: string | null;
  paymentHash: string | null;
  payerPubkey: string;
  recipientName: string;
  recipientPubkey: string;
  targetEventId: string | null;
  targetEventKind: number | null;
};

const HISTORY_LIMIT = 500;
const HISTORY_EVENT = "buzz:wallet-zap-history-updated";

function legacyStorageKey(ownerPubkey: string) {
  return `buzz-wallet-zap-history.v1:${ownerPubkey.trim().toLowerCase()}`;
}

function storageKey(ownerPubkey: string, relayUrl: string) {
  return `${legacyStorageKey(ownerPubkey)}:${encodeURIComponent(
    relayUrl.trim().replace(/\/$/, "").toLowerCase(),
  )}`;
}

function parseZapHistoryItem(value: unknown): ZapHistoryItem | null {
  if (!value || typeof value !== "object") return null;
  const item = value as Partial<ZapHistoryItem>;
  if (
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
  ) {
    const paymentHash =
      typeof item.paymentHash === "string" &&
      /^[0-9a-f]{64}$/i.test(item.paymentHash)
        ? item.paymentHash.toLowerCase()
        : null;
    return {
      amount: item.amount,
      channelId:
        item.channelId === null || typeof item.channelId === "string"
          ? item.channelId
          : null,
      comment: item.comment,
      createdAt: item.createdAt,
      eventId: item.eventId,
      intentEventId: item.intentEventId,
      leaseId:
        item.leaseId === null || typeof item.leaseId === "string"
          ? item.leaseId
          : null,
      paymentHash,
      payerPubkey: item.payerPubkey,
      recipientName: item.recipientName,
      recipientPubkey: item.recipientPubkey,
      targetEventId: item.targetEventId,
      targetEventKind:
        item.targetEventKind === null ||
        (typeof item.targetEventKind === "number" &&
          Number.isSafeInteger(item.targetEventKind) &&
          item.targetEventKind >= 0)
          ? item.targetEventKind
          : null,
    };
  }
  return null;
}

export function parseZapHistory(raw: string | null): ZapHistoryItem[] {
  try {
    const value = JSON.parse(raw ?? "[]");
    if (!Array.isArray(value)) return [];
    return value
      .map(parseZapHistoryItem)
      .filter((item): item is ZapHistoryItem => item !== null)
      .sort((left, right) => right.createdAt - left.createdAt)
      .slice(0, HISTORY_LIMIT);
  } catch {
    return [];
  }
}

export function addZapHistoryItem(
  existing: readonly ZapHistoryItem[],
  item: ZapHistoryItem,
): { didAdd: boolean; didUpdate: boolean; items: ZapHistoryItem[] } {
  const existingIndex = existing.findIndex(
    (candidate) => candidate.eventId === item.eventId,
  );
  if (existingIndex >= 0) {
    const current = existing[existingIndex];
    const enriched = {
      ...current,
      channelId: current.channelId ?? item.channelId,
      leaseId: current.leaseId ?? item.leaseId,
      paymentHash: current.paymentHash ?? item.paymentHash,
      recipientName: current.recipientName.trim()
        ? current.recipientName
        : item.recipientName,
      targetEventId: current.targetEventId ?? item.targetEventId,
      targetEventKind: current.targetEventKind ?? item.targetEventKind,
    };
    if (
      enriched.channelId !== current.channelId ||
      enriched.leaseId !== current.leaseId ||
      enriched.paymentHash !== current.paymentHash ||
      enriched.recipientName !== current.recipientName ||
      enriched.targetEventId !== current.targetEventId ||
      enriched.targetEventKind !== current.targetEventKind
    ) {
      const items = [...existing];
      items[existingIndex] = enriched;
      return { didAdd: false, didUpdate: true, items };
    }
    return { didAdd: false, didUpdate: false, items: [...existing] };
  }
  return {
    didAdd: true,
    didUpdate: false,
    items: [item, ...existing]
      .sort((left, right) => right.createdAt - left.createdAt)
      .slice(0, HISTORY_LIMIT),
  };
}

export function readZapHistory(
  ownerPubkey: string,
  relayUrl: string,
): ZapHistoryItem[] {
  if (typeof window === "undefined" || !ownerPubkey.trim() || !relayUrl.trim())
    return [];
  try {
    const key = storageKey(ownerPubkey, relayUrl);
    const scoped = localStorage.getItem(key);
    if (scoped !== null) return parseZapHistory(scoped);

    // Adopt the old unscoped cache once. Future writes remain relay-scoped.
    const legacy = parseZapHistory(
      localStorage.getItem(legacyStorageKey(ownerPubkey)),
    );
    if (legacy.length > 0) localStorage.setItem(key, JSON.stringify(legacy));
    localStorage.removeItem(legacyStorageKey(ownerPubkey));
    return legacy;
  } catch {
    return [];
  }
}

export function persistZapHistoryItem(
  ownerPubkey: string,
  relayUrl: string,
  item: ZapHistoryItem,
): "added" | "duplicate" | "failed" | "updated" {
  if (typeof window === "undefined" || !ownerPubkey.trim() || !relayUrl.trim())
    return "failed";
  const result = addZapHistoryItem(readZapHistory(ownerPubkey, relayUrl), item);
  if (!result.didAdd && !result.didUpdate) return "duplicate";
  try {
    localStorage.setItem(
      storageKey(ownerPubkey, relayUrl),
      JSON.stringify(result.items),
    );
  } catch {
    return "failed";
  }
  if (typeof window.dispatchEvent === "function") {
    window.dispatchEvent(
      new CustomEvent(HISTORY_EVENT, {
        detail: {
          ownerPubkey: ownerPubkey.trim().toLowerCase(),
          relayUrl: relayUrl.trim().replace(/\/$/, "").toLowerCase(),
        },
      }),
    );
  }
  return result.didAdd ? "added" : "updated";
}

export function useZapHistory(
  ownerPubkey: string | undefined,
  relayUrl: string | undefined,
) {
  const normalizedOwner = ownerPubkey?.trim().toLowerCase() ?? "";
  const normalizedRelay =
    relayUrl?.trim().replace(/\/$/, "").toLowerCase() ?? "";
  const [items, setItems] = React.useState<ZapHistoryItem[]>(() =>
    readZapHistory(normalizedOwner, normalizedRelay),
  );

  React.useEffect(() => {
    setItems(readZapHistory(normalizedOwner, normalizedRelay));
    function refresh(event: Event) {
      if (
        event instanceof CustomEvent &&
        ((event.detail?.ownerPubkey &&
          event.detail.ownerPubkey !== normalizedOwner) ||
          (event.detail?.relayUrl && event.detail.relayUrl !== normalizedRelay))
      ) {
        return;
      }
      setItems(readZapHistory(normalizedOwner, normalizedRelay));
    }
    function refreshFromStorage(event: StorageEvent) {
      if (event.key === storageKey(normalizedOwner, normalizedRelay)) {
        refresh(event);
      }
    }
    window.addEventListener(HISTORY_EVENT, refresh);
    window.addEventListener("storage", refreshFromStorage);
    return () => {
      window.removeEventListener(HISTORY_EVENT, refresh);
      window.removeEventListener("storage", refreshFromStorage);
    };
  }, [normalizedOwner, normalizedRelay]);

  return items;
}

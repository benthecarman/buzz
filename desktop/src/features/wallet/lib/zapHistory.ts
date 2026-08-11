import * as React from "react";

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

function legacyStorageKey(ownerPubkey: string) {
  return `buzz-wallet-zap-history.v1:${ownerPubkey.trim().toLowerCase()}`;
}

function storageKey(ownerPubkey: string, relayUrl: string) {
  return `${legacyStorageKey(ownerPubkey)}:${encodeURIComponent(
    relayUrl.trim().replace(/\/$/, "").toLowerCase(),
  )}`;
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
): "added" | "duplicate" | "failed" {
  if (typeof window === "undefined" || !ownerPubkey.trim() || !relayUrl.trim())
    return "failed";
  const result = addZapHistoryItem(readZapHistory(ownerPubkey, relayUrl), item);
  if (!result.didAdd) return "duplicate";
  try {
    localStorage.setItem(
      storageKey(ownerPubkey, relayUrl),
      JSON.stringify(result.items),
    );
  } catch {
    return "failed";
  }
  window.dispatchEvent(
    new CustomEvent(HISTORY_EVENT, {
      detail: {
        ownerPubkey: ownerPubkey.trim().toLowerCase(),
        relayUrl: relayUrl.trim().replace(/\/$/, "").toLowerCase(),
      },
    }),
  );
  return "added";
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
      if (event.key === storageKey(normalizedOwner, normalizedRelay))
        refresh(event);
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

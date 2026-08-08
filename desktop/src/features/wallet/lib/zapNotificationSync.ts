import type { RelaySubscriptionFilter } from "@/shared/api/relayClientShared";
import type { RelayEvent } from "@/shared/api/types";
import { KIND_BOLT12_ZAP } from "@/shared/constants/kinds";
import type { WalletTransactionPage } from "../types";

// v3 replays proofs whose cursor was advanced before Lexe indexed the inbound
// payment, leaving received-zap history permanently empty.
const CURSOR_STORAGE_PREFIX = "buzz-wallet-zap-sync.v3";
export const ZAP_SYNC_OVERLAP_SECONDS = 5;
export const ZAP_SYNC_PAGE_LIMIT = 500;
const MAX_ZAP_SYNC_PAGES = 10_000;
const MAX_WALLET_TRANSACTION_PAGES = 500;

export type ZapSyncScope = {
  ownerPubkey: string;
  recipientPubkey: string;
  relayUrl: string;
};

export type ZapCatchupOutcome = {
  createdAt: number;
  status: "processed" | "retry";
};

/**
 * Advance through a catch-up batch without skipping the first unresolved proof.
 * Later proofs may still be processed and persisted while the cursor remains
 * pinned close enough for the unresolved proof to be replayed.
 */
export function zapCatchupProgress(
  currentCursor: number,
  outcomes: readonly ZapCatchupOutcome[],
): { cursor: number; hasPending: boolean } {
  let cursor = currentCursor;
  for (const outcome of outcomes) {
    cursor = Math.max(cursor, outcome.createdAt);
    if (outcome.status === "retry") {
      return { cursor, hasPending: true };
    }
  }
  return { cursor, hasPending: false };
}

function normalizedScopePart(value: string) {
  return encodeURIComponent(value.trim().toLowerCase());
}

export function zapSyncCursorStorageKey(scope: ZapSyncScope) {
  return [
    CURSOR_STORAGE_PREFIX,
    normalizedScopePart(scope.relayUrl.replace(/\/$/, "")),
    normalizedScopePart(scope.ownerPubkey),
    normalizedScopePart(scope.recipientPubkey),
  ].join(":");
}

export function parseZapSyncCursor(raw: string | null): number {
  if (raw === null) return 0;
  const cursor = Number(raw);
  return Number.isSafeInteger(cursor) && cursor >= 0 ? cursor : 0;
}

export function readZapSyncCursor(scope: ZapSyncScope): number {
  if (typeof window === "undefined") return 0;
  try {
    return parseZapSyncCursor(
      window.localStorage.getItem(zapSyncCursorStorageKey(scope)),
    );
  } catch {
    return 0;
  }
}

export function writeZapSyncCursor(
  scope: ZapSyncScope,
  createdAt: number,
): boolean {
  if (
    typeof window === "undefined" ||
    !Number.isSafeInteger(createdAt) ||
    createdAt < 0
  ) {
    return false;
  }
  try {
    const key = zapSyncCursorStorageKey(scope);
    const next = Math.max(
      parseZapSyncCursor(window.localStorage.getItem(key)),
      createdAt,
    );
    window.localStorage.setItem(key, String(next));
    return true;
  } catch {
    return false;
  }
}

export function buildZapCatchupFilter(
  recipientPubkey: string,
  since: number,
  until: number,
): RelaySubscriptionFilter {
  return {
    kinds: [KIND_BOLT12_ZAP],
    "#p": [recipientPubkey.trim().toLowerCase()],
    limit: ZAP_SYNC_PAGE_LIMIT,
    since: Math.max(0, since - ZAP_SYNC_OVERLAP_SECONDS),
    until,
  };
}

/** Fetch every stored recipient zap between a durable cursor and this sync. */
export async function fetchZapCatchupEvents(input: {
  recipientPubkey: string;
  since: number;
  until: number;
  fetchPage: (filter: RelaySubscriptionFilter) => Promise<RelayEvent[]>;
}): Promise<RelayEvent[]> {
  const eventsById = new Map<string, RelayEvent>();
  const replaySince = Math.max(0, input.since - ZAP_SYNC_OVERLAP_SECONDS);
  let pageUntil = input.until;

  for (let page = 0; page < MAX_ZAP_SYNC_PAGES; page += 1) {
    const events = await input.fetchPage(
      buildZapCatchupFilter(input.recipientPubkey, input.since, pageUntil),
    );
    for (const event of events) eventsById.set(event.id, event);
    if (events.length < ZAP_SYNC_PAGE_LIMIT) {
      return [...eventsById.values()].sort(
        (left, right) =>
          left.created_at - right.created_at || left.id.localeCompare(right.id),
      );
    }

    const oldestCreatedAt = events.reduce(
      (oldest, event) => Math.min(oldest, event.created_at),
      pageUntil,
    );
    if (oldestCreatedAt <= replaySince) {
      return [...eventsById.values()].sort(
        (left, right) =>
          left.created_at - right.created_at || left.id.localeCompare(right.id),
      );
    }
    pageUntil =
      oldestCreatedAt < pageUntil ? oldestCreatedAt : oldestCreatedAt - 1;
  }

  throw new Error("Zap history exceeded the page safety limit.");
}

/** Correlate a proof against the complete inbound wallet transaction history. */
export async function hasSettledZapPayment(input: {
  amount: number;
  intentEventId: string;
  listTransactions: (
    cursor?: string,
    sync?: boolean,
  ) => Promise<WalletTransactionPage>;
}): Promise<boolean> {
  let cursor: string | undefined;
  const seenCursors = new Set<string>();
  const expectedPayerNote = `nostr:nipB1:${input.intentEventId}`;

  for (
    let pageIndex = 0;
    pageIndex < MAX_WALLET_TRANSACTION_PAGES;
    pageIndex += 1
  ) {
    const page = await input.listTransactions(cursor, cursor === undefined);
    if (
      page.transactions.some(
        (transaction) =>
          transaction.direction === "inbound" &&
          transaction.status === "completed" &&
          transaction.amount === input.amount &&
          (transaction.payerNote === expectedPayerNote ||
            transaction.note === expectedPayerNote),
      )
    ) {
      return true;
    }
    if (!page.nextCursor) return false;
    if (seenCursors.has(page.nextCursor)) {
      throw new Error("Wallet transaction history repeated a cursor.");
    }
    seenCursors.add(page.nextCursor);
    cursor = page.nextCursor;
  }

  throw new Error("Wallet transaction history exceeded the page safety limit.");
}

import type { AgentRuntimeCapMinutes } from "@/features/agents/runtimePayments";

export const AGENT_RUNTIME_CHECKOUT_STORAGE_PREFIX =
  "buzz.agent-runtime-checkout.v2";

export type StoredAgentRuntimeCheckoutRow = {
  pubkey: string;
  name: string;
  rateSats: number;
  availableMs: number;
  zapIdempotencyKey: string;
  paymentSent: boolean;
  reservationTag: string[] | null;
};

export type StoredAgentRuntimeCheckout = {
  version: 2;
  channelId: string;
  capMinutes: AgentRuntimeCapMinutes;
  updatedAtMs: number;
  rows: StoredAgentRuntimeCheckoutRow[];
};

function storageKey(scopeId: string): string {
  return `${AGENT_RUNTIME_CHECKOUT_STORAGE_PREFIX}:${scopeId}`;
}

function isCapMinutes(value: unknown): value is AgentRuntimeCapMinutes {
  return value === 15 || value === 30 || value === 60;
}

function isRow(value: unknown): value is StoredAgentRuntimeCheckoutRow {
  if (!value || typeof value !== "object") return false;
  const row = value as Partial<StoredAgentRuntimeCheckoutRow>;
  return (
    typeof row.pubkey === "string" &&
    /^[0-9a-f]{64}$/u.test(row.pubkey) &&
    typeof row.name === "string" &&
    Number.isSafeInteger(row.rateSats) &&
    Number(row.rateSats) > 0 &&
    Number.isSafeInteger(row.availableMs) &&
    Number(row.availableMs) >= 0 &&
    typeof row.zapIdempotencyKey === "string" &&
    row.zapIdempotencyKey.length > 0 &&
    typeof row.paymentSent === "boolean" &&
    (row.reservationTag === null ||
      (Array.isArray(row.reservationTag) &&
        row.reservationTag.length === 3 &&
        row.reservationTag.every((tag) => typeof tag === "string")))
  );
}

export function loadAgentRuntimeCheckout(
  scopeId: string,
  storage: Storage = window.localStorage,
): StoredAgentRuntimeCheckout | null {
  try {
    const encoded = storage.getItem(storageKey(scopeId));
    if (!encoded) return null;
    const value = JSON.parse(encoded) as Partial<StoredAgentRuntimeCheckout>;
    if (
      value.version !== 2 ||
      typeof value.channelId !== "string" ||
      !isCapMinutes(value.capMinutes) ||
      !Number.isSafeInteger(value.updatedAtMs) ||
      !Array.isArray(value.rows) ||
      value.rows.length === 0 ||
      !value.rows.every(isRow)
    ) {
      storage.removeItem(storageKey(scopeId));
      return null;
    }
    return value as StoredAgentRuntimeCheckout;
  } catch {
    return null;
  }
}

export function saveAgentRuntimeCheckout(
  scopeId: string,
  checkout: Omit<StoredAgentRuntimeCheckout, "version" | "updatedAtMs">,
  storage: Storage = window.localStorage,
): StoredAgentRuntimeCheckout {
  const stored: StoredAgentRuntimeCheckout = {
    ...checkout,
    version: 2,
    updatedAtMs: Date.now(),
  };
  storage.setItem(storageKey(scopeId), JSON.stringify(stored));
  return stored;
}

export function clearAgentRuntimeCheckout(
  scopeId: string,
  storage: Storage = window.localStorage,
): void {
  storage.removeItem(storageKey(scopeId));
}

export const AGENT_RUNTIME_CHECKOUT_STORAGE_PREFIX =
  "buzz.agent-runtime-checkout.v4";

export type StoredAgentRuntimeCheckoutRow = {
  pubkey: string;
  name: string;
  ownerPubkey: string | null;
  priceSats: number;
  invocationWindowSeconds: number;
  zapIdempotencyKey: string;
  zapEventId: string | null;
  validUntilSeconds: number | null;
};

export type StoredAgentRuntimeCheckout = {
  version: 4;
  channelId: string;
  updatedAtMs: number;
  rows: StoredAgentRuntimeCheckoutRow[];
};

function storageKey(scopeId: string): string {
  return `${AGENT_RUNTIME_CHECKOUT_STORAGE_PREFIX}:${scopeId}`;
}

function isRow(value: unknown): value is StoredAgentRuntimeCheckoutRow {
  if (!value || typeof value !== "object") return false;
  const row = value as Partial<StoredAgentRuntimeCheckoutRow>;
  return (
    typeof row.pubkey === "string" &&
    /^[0-9a-f]{64}$/u.test(row.pubkey) &&
    typeof row.name === "string" &&
    (row.ownerPubkey === null || typeof row.ownerPubkey === "string") &&
    Number.isSafeInteger(row.priceSats) &&
    Number(row.priceSats) > 0 &&
    row.invocationWindowSeconds === 300 &&
    typeof row.zapIdempotencyKey === "string" &&
    row.zapIdempotencyKey.length > 0 &&
    (row.zapEventId === null ||
      (typeof row.zapEventId === "string" &&
        /^[0-9a-f]{64}$/u.test(row.zapEventId))) &&
    (row.validUntilSeconds === null ||
      (Number.isSafeInteger(row.validUntilSeconds) &&
        Number(row.validUntilSeconds) > 0))
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
      value.version !== 4 ||
      typeof value.channelId !== "string" ||
      !Number.isSafeInteger(value.updatedAtMs) ||
      !Array.isArray(value.rows) ||
      !value.rows.every(isRow)
    ) {
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
): void {
  storage.setItem(
    storageKey(scopeId),
    JSON.stringify({ ...checkout, version: 4, updatedAtMs: Date.now() }),
  );
}

export function activeStoredRuntimeZap(
  row: StoredAgentRuntimeCheckoutRow | undefined,
  nowSeconds: number,
): string | null {
  if (
    !row?.zapEventId ||
    row.validUntilSeconds === null ||
    nowSeconds > row.validUntilSeconds
  ) {
    return null;
  }
  return row.zapEventId;
}

export function clearAgentRuntimeCheckout(
  scopeId: string,
  storage: Storage = window.localStorage,
): void {
  storage.removeItem(storageKey(scopeId));
}

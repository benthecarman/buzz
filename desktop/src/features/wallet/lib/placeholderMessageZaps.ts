import { useQuery } from "@tanstack/react-query";

import { listPlaceholderMessageZaps } from "../api";
import type { WalletPlaceholderMessageZap } from "../types";

export const placeholderMessageZapsQueryKey = [
  "wallet",
  "placeholder-message-zaps",
] as const;

/**
 * Return local settled payments for one message and payer identity.
 *
 * These are UI placeholders, not NIP-B1 proofs. They are never published.
 */
export function usePlaceholderMessageZaps(
  targetEventId: string,
  payerPubkey: string | undefined,
): WalletPlaceholderMessageZap[] {
  const query = useQuery({
    queryKey: [...placeholderMessageZapsQueryKey, payerPubkey],
    queryFn: listPlaceholderMessageZaps,
    enabled: Boolean(payerPubkey),
    staleTime: Number.POSITIVE_INFINITY,
  });
  return (query.data ?? []).filter(
    (receipt) => receipt.targetEventId === targetEventId,
  );
}

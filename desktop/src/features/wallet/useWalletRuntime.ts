import type { Channel } from "@/shared/api/types";
import { useHostedAgentOwnership } from "./useHostedAgentOwnership";
import { useIncomingWalletPayments } from "./useIncomingWalletPayments";

/** Keep wallet background work active outside the wallet panel. */
export function useWalletRuntime(
  buyerPubkey: string | undefined,
  channels: Channel[],
): void {
  useIncomingWalletPayments();
  useHostedAgentOwnership(buyerPubkey, channels);
}

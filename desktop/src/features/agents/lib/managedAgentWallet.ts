import { loadCommunities } from "@/features/communities/communityStorage";
import { getFeature } from "@/shared/features/manifest";
import { resolveEnabled } from "@/shared/features/resolveEnabled";
import { getOverrides } from "@/shared/features/store";

export function managedAgentWalletCreationContext() {
  const bitcoin = getFeature("bitcoin");
  return {
    walletEnabled: bitcoin
      ? resolveEnabled(bitcoin.id, getOverrides(), bitcoin.defaultEnabled)
      : true,
    walletRelayUrls: loadCommunities().map((community) => community.relayUrl),
  };
}

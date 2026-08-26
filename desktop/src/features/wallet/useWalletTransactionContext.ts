import { useQuery } from "@tanstack/react-query";
import * as React from "react";

import {
  useManagedAgentsQuery,
  useRelayAgentsQuery,
} from "@/features/agents/hooks";
import { mergeOwnedAgentPubkeys } from "@/features/agents/knownAgentPubkeys";
import { useChannelsQuery } from "@/features/channels/hooks";
import { useCommunities } from "@/features/communities/useCommunities";
import { useUsersBatchQuery } from "@/features/profile/hooks";
import { resolveUserLabel } from "@/features/profile/lib/identity";
import { relayClient } from "@/shared/api/relayClient";
import { useIdentityQuery } from "@/shared/api/hooks";
import {
  CHANNEL_TIMELINE_CONTENT_KINDS,
  KIND_FORUM_COMMENT,
  KIND_FORUM_POST,
  KIND_TEXT_NOTE,
} from "@/shared/constants/kinds";
import { useZapHistory } from "./lib/zapHistory";
import type { WalletTransactionContext } from "./lib/walletTransactionPresentation";

const TARGET_EVENT_KINDS = [
  ...CHANNEL_TIMELINE_CONTENT_KINDS,
  KIND_FORUM_POST,
  KIND_FORUM_COMMENT,
  KIND_TEXT_NOTE,
];

/** Resolve cached zap proofs and their existing profile and channel labels. */
export function useWalletTransactionContext(): WalletTransactionContext {
  const ownerPubkey = useIdentityQuery().data?.pubkey;
  const { activeCommunity } = useCommunities();
  const managedAgents = useManagedAgentsQuery().data;
  const relayAgents = useRelayAgentsQuery().data;
  const zaps = useZapHistory(ownerPubkey, activeCommunity?.relayUrl);
  const channels = useChannelsQuery().data ?? [];
  const userPubkeys = React.useMemo(
    () => [
      ...new Set(zaps.flatMap((zap) => [zap.payerPubkey, zap.recipientPubkey])),
    ],
    [zaps],
  );
  const profiles = useUsersBatchQuery(userPubkeys).data?.profiles;
  const ownedAgentPubkeys = React.useMemo(
    () =>
      mergeOwnedAgentPubkeys(managedAgents, profiles, ownerPubkey, relayAgents),
    [managedAgents, ownerPubkey, profiles, relayAgents],
  );
  const targetEventIds = React.useMemo(
    () =>
      [
        ...new Set(
          zaps.flatMap((zap) => (zap.targetEventId ? [zap.targetEventId] : [])),
        ),
      ].sort(),
    [zaps],
  );
  const targetEventsQuery = useQuery({
    enabled: targetEventIds.length > 0,
    queryKey: ["wallet-zap-target-events", ...targetEventIds],
    queryFn: () =>
      relayClient.fetchEvents({
        ids: targetEventIds,
        kinds: [...new Set(TARGET_EVENT_KINDS)],
        limit: Math.max(1, targetEventIds.length),
      }),
    staleTime: Number.POSITIVE_INFINITY,
  });

  return React.useMemo(
    () => ({
      channelNames: new Map(
        channels.map((channel) => [channel.id, channel.name]),
      ),
      ownedAgentPubkeys,
      ownerPubkey: ownerPubkey?.trim().toLowerCase() ?? null,
      targetEvents: new Map(
        (targetEventsQuery.data ?? []).map((event) => [event.id, event]),
      ),
      userNames: new Map(
        userPubkeys.map((pubkey) => [
          pubkey.trim().toLowerCase(),
          resolveUserLabel({ pubkey, profiles }),
        ]),
      ),
      zapsByIntent: new Map(zaps.map((zap) => [zap.intentEventId, zap])),
      zapsByPaymentHash: new Map(
        zaps.flatMap((zap) =>
          zap.paymentHash
            ? [[zap.paymentHash.toLowerCase(), zap] as const]
            : [],
        ),
      ),
    }),
    [
      channels,
      ownedAgentPubkeys,
      ownerPubkey,
      profiles,
      targetEventsQuery.data,
      userPubkeys,
      zaps,
    ],
  );
}

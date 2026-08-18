import {
  filterAdmittedMentionPubkeys,
  getAgentMentionAdmission,
  getControlledHostedAgentPubkeys,
  getMentionableAgentPubkeys,
  type AgentEligibilityScope,
} from "@/features/agents/lib/agentAutocompleteEligibility";
import { evictUsersBatchEntries } from "@/features/profile/hooks";
import { getUsersBatch } from "@/shared/api/tauriProfiles";
import type {
  ManagedAgent,
  RelayAgent,
  UsersBatchResponse,
} from "@/shared/api/types";
import { normalizePubkey } from "@/shared/lib/pubkey";
import { useQueryClient } from "@tanstack/react-query";
import * as React from "react";

type DirectoryResult<T> = {
  data: T | undefined;
  error: Error | null;
};

export async function revalidateAgentMentionPubkeys({
  pubkeys,
  agentPubkeys,
  channelAgentPubkeys,
  currentPubkey,
  eligibilityScope,
  sharedChannelIds,
  ownerOnly,
  ownerPolicyError,
  refetchManagedAgents,
  refetchRelayAgents,
  refetchOwnerProfiles,
}: {
  pubkeys: readonly string[];
  agentPubkeys: ReadonlySet<string>;
  channelAgentPubkeys: ReadonlySet<string>;
  currentPubkey: string | null;
  eligibilityScope: AgentEligibilityScope;
  sharedChannelIds: ReadonlySet<string>;
  ownerOnly: boolean | undefined;
  ownerPolicyError: Error | null;
  refetchManagedAgents: () => Promise<DirectoryResult<ManagedAgent[]>>;
  refetchRelayAgents: () => Promise<DirectoryResult<RelayAgent[]>>;
  refetchOwnerProfiles: (pubkeys: string[]) => Promise<UsersBatchResponse>;
}) {
  const requestedAgentPubkeys = new Set(
    pubkeys.map(normalizePubkey).filter((pubkey) => agentPubkeys.has(pubkey)),
  );
  if (requestedAgentPubkeys.size === 0) {
    return [...pubkeys];
  }

  const needsAgentProfiles =
    ownerOnly ||
    [...requestedAgentPubkeys].some((pubkey) =>
      channelAgentPubkeys.has(pubkey),
    );
  const [managedResult, relayResult, agentProfiles] = await Promise.all([
    refetchManagedAgents(),
    refetchRelayAgents(),
    needsAgentProfiles
      ? refetchOwnerProfiles([...requestedAgentPubkeys]).catch(() => null)
      : Promise.resolve(null),
  ]);
  const relayDirectoryReady =
    relayResult.error === null && relayResult.data !== undefined;
  if (
    ownerOnly === undefined ||
    ownerPolicyError !== null ||
    managedResult.error !== null ||
    managedResult.data === undefined
  ) {
    return filterAdmittedMentionPubkeys(pubkeys, agentPubkeys, new Set());
  }

  const managedPubkeys = new Set(
    managedResult.data.map((agent) => normalizePubkey(agent.pubkey)),
  );
  const controlledHostedAgentPubkeys = getControlledHostedAgentPubkeys({
    currentPubkey,
    members: [...channelAgentPubkeys].map((pubkey) => ({
      pubkey,
      isAgent: true,
    })),
    getManagerPubkey: (pubkey) =>
      agentProfiles?.profiles[pubkey]?.managerPubkey,
  });
  const mentionablePubkeys = getMentionableAgentPubkeys({
    currentPubkey,
    eligibilityScope,
    managedAgentPubkeys: managedPubkeys,
    controlledHostedAgentPubkeys,
    relayAgents: relayDirectoryReady ? relayResult.data : [],
    sharedChannelIds,
  });
  const admittedPubkeys = new Set(
    [...agentPubkeys].filter((pubkey) => {
      const isManagedAgent = managedPubkeys.has(normalizePubkey(pubkey));
      const isControlledHostedAgent = controlledHostedAgentPubkeys.has(
        normalizePubkey(pubkey),
      );
      const directoryReady =
        isManagedAgent ||
        isControlledHostedAgent ||
        (relayDirectoryReady && (!ownerOnly || agentProfiles !== null));
      return (
        getAgentMentionAdmission({
          isAgent: true,
          isManagedAgent,
          isControlledHostedAgent,
          pubkey,
          ownerPubkey: agentProfiles?.profiles[pubkey]?.ownerPubkey,
          currentPubkey,
          mentionableAgentPubkeys: mentionablePubkeys,
          directoryReady,
          ownerOnly,
        }) === "allow"
      );
    }),
  );
  return filterAdmittedMentionPubkeys(pubkeys, agentPubkeys, admittedPubkeys);
}

export function useAgentMentionRevalidation({
  agentPubkeys,
  channelAgentPubkeys,
  getSelectedAgentPubkeys,
  currentPubkey,
  eligibilityScope,
  sharedChannelIds,
  ownerOnly,
  ownerPolicyError,
  refetchManagedAgents,
  refetchRelayAgents,
}: {
  agentPubkeys: ReadonlySet<string>;
  channelAgentPubkeys: ReadonlySet<string>;
  getSelectedAgentPubkeys: () => ReadonlySet<string>;
  currentPubkey: string | null;
  eligibilityScope: AgentEligibilityScope;
  sharedChannelIds: ReadonlySet<string>;
  ownerOnly: boolean | undefined;
  ownerPolicyError: Error | null;
  refetchManagedAgents: () => Promise<DirectoryResult<ManagedAgent[]>>;
  refetchRelayAgents: () => Promise<DirectoryResult<RelayAgent[]>>;
}) {
  const queryClient = useQueryClient();
  const refetchOwnerProfiles = React.useCallback(
    async (pubkeys: string[]) => {
      evictUsersBatchEntries(queryClient, pubkeys);
      return getUsersBatch(pubkeys);
    },
    [queryClient],
  );
  return React.useCallback(
    (pubkeys: readonly string[]) =>
      revalidateAgentMentionPubkeys({
        pubkeys,
        agentPubkeys: new Set([...agentPubkeys, ...getSelectedAgentPubkeys()]),
        channelAgentPubkeys,
        currentPubkey,
        eligibilityScope,
        sharedChannelIds,
        ownerOnly,
        ownerPolicyError,
        refetchManagedAgents,
        refetchRelayAgents,
        refetchOwnerProfiles,
      }),
    [
      agentPubkeys,
      channelAgentPubkeys,
      currentPubkey,
      eligibilityScope,
      getSelectedAgentPubkeys,
      ownerOnly,
      ownerPolicyError,
      refetchManagedAgents,
      refetchOwnerProfiles,
      refetchRelayAgents,
      sharedChannelIds,
    ],
  );
}

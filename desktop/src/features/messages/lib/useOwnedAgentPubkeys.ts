import * as React from "react";

import { getOwnedAgentPubkeys } from "@/features/agents/lib/agentAutocompleteEligibility";
import type { UserProfileLookup } from "@/features/profile/lib/identity";
import type {
  ChannelMember,
  RelayAgent,
  UserSearchResult,
} from "@/shared/api/types";

export function useOwnedAgentPubkeys({
  currentPubkey,
  members,
  profiles,
  relayAgents,
  userSearchResults,
}: {
  currentPubkey: string | null;
  members: ChannelMember[] | undefined;
  profiles: UserProfileLookup | undefined;
  relayAgents: RelayAgent[] | undefined;
  userSearchResults: UserSearchResult[];
}) {
  return React.useMemo(
    () =>
      getOwnedAgentPubkeys({
        currentPubkey,
        members: members ?? [],
        getOwnerPubkey: (pubkey) => profiles?.[pubkey]?.ownerPubkey,
        additionalAgents: [
          ...(relayAgents ?? []),
          ...userSearchResults.filter((user) => user.isAgent),
        ],
      }),
    [currentPubkey, members, profiles, relayAgents, userSearchResults],
  );
}

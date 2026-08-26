import * as React from "react";

import { useUsersBatchQuery } from "@/features/profile/hooks";
import type { UserProfileLookup } from "@/features/profile/lib/identity";

/** Fetch agent owner profiles for message surfaces. */
export function useMessageOwnerProfiles(profiles: UserProfileLookup) {
  const ownerPubkeys = React.useMemo(
    () => [
      ...new Set(
        Object.values(profiles)
          .map((profile) => profile.ownerPubkey)
          .filter((pubkey): pubkey is string => Boolean(pubkey)),
      ),
    ],
    [profiles],
  );
  const ownerProfilesQuery = useUsersBatchQuery(ownerPubkeys, {
    enabled: ownerPubkeys.length > 0,
  });
  return ownerProfilesQuery.data?.profiles;
}

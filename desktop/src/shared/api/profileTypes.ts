export type Profile = {
  pubkey: string;
  displayName: string | null;
  avatarUrl: string | null;
  about: string | null;
  nip05Handle: string | null;
  ownerPubkey: string | null;
  /** Host-verified buyer who manages a hosted agent. This does not grant
   * NIP-OA owner permissions. */
  /** True when a real kind:0 metadata event exists on the relay for this pubkey.
   * False for the synthesized fallback returned when no event is present.
   * Used by the onboarding gate to distinguish new users from returning users
   * whose display name happens to be empty. */
  hasProfileEvent: boolean;
};

export type UserProfileSummary = {
  displayName: string | null;
  /** Kind-0 `name` field, kept separate from `displayName` so @mention text
   * can be matched against either alias (agents/CLI resolve mentions against
   * `display_name` *or* `name` at send time). */
  name?: string | null;
  avatarUrl: string | null;
  nip05Handle: string | null;
  ownerPubkey: string | null;
  isAgent?: boolean;
};

export type UsersBatchResponse = {
  profiles: Record<string, UserProfileSummary>;
  missing: string[];
};

export type UserSearchResult = {
  pubkey: string;
  displayName: string | null;
  avatarUrl: string | null;
  nip05Handle: string | null;
  ownerPubkey: string | null;
  isAgent: boolean;
};

export type UserSearchPage = {
  users: UserSearchResult[];
  nextCursor: string | null;
};

export type UpdateProfileInput = {
  displayName?: string;
  avatarUrl?: string;
  about?: string;
  nip05Handle?: string;
};

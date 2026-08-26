import * as React from "react";
import { filterSelectedMentionPubkeys } from "@/features/agents/lib/agentAutocompleteEligibility";
import type { AgentPersona } from "@/shared/api/types";
import { extractMentionPersonasFromMaps } from "./extractMentionPersonas";
import { extractMentionPubkeys } from "./extractMentionPubkeys";
import type { MentionCandidate } from "./mentionCandidates";

type MutableRef<T> = { current: T };

export function useMentionExtraction({
  activePersonaById,
  admittedAgentPubkeys,
  agentIdentityPubkeys,
  mentionCandidates,
  mentionMapRef,
  personaMentionMapRef,
  selectedAgentMentionPubkeysRef,
}: {
  activePersonaById: ReadonlyMap<string, AgentPersona>;
  admittedAgentPubkeys: ReadonlySet<string>;
  agentIdentityPubkeys: ReadonlySet<string>;
  mentionCandidates: readonly MentionCandidate[];
  mentionMapRef: MutableRef<Map<string, string>>;
  personaMentionMapRef: MutableRef<Map<string, string>>;
  selectedAgentMentionPubkeysRef: MutableRef<Set<string>>;
}) {
  const extractMentionPubkeysForCurrentMentions = React.useCallback(
    (text: string): string[] => {
      const extracted = extractMentionPubkeys({
        text,
        selectedMentions: mentionMapRef.current,
        selectedDisplayNames: personaMentionMapRef.current.keys(),
        memberCandidates: mentionCandidates,
      });
      return filterSelectedMentionPubkeys(
        extracted,
        agentIdentityPubkeys,
        admittedAgentPubkeys,
        selectedAgentMentionPubkeysRef.current,
      );
    },
    [
      admittedAgentPubkeys,
      agentIdentityPubkeys,
      mentionCandidates,
      mentionMapRef,
      personaMentionMapRef,
      selectedAgentMentionPubkeysRef,
    ],
  );
  const extractMentionPersonas = React.useCallback(
    (text: string) =>
      extractMentionPersonasFromMaps(
        text,
        personaMentionMapRef.current,
        activePersonaById,
      ),
    [activePersonaById, personaMentionMapRef],
  );

  return { extractMentionPersonas, extractMentionPubkeysForCurrentMentions };
}

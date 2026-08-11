import * as React from "react";

import {
  mergeMessages,
  useChannelRuntimeZapsQuery,
} from "@/features/messages/hooks";
import { useVerifiedZapEvents } from "@/features/wallet/lib/useVerifiedZapEvents";
import type { Channel, RelayEvent } from "@/shared/api/types";
import { useMessageEventProfilePubkeys } from "./useMessageEventProfilePubkeys";

const EMPTY_RELAY_EVENTS: RelayEvent[] = [];

export function useChannelRuntimeZapEvents(
  channel: Channel | null,
  messages: RelayEvent[],
  threadReplies: RelayEvent[],
  relaySelfPubkey: string | null | undefined,
) {
  const runtimeZapsQuery = useChannelRuntimeZapsQuery(channel);
  const timelineEvents = React.useMemo(
    () =>
      (runtimeZapsQuery.data ?? EMPTY_RELAY_EVENTS).reduce(
        mergeMessages,
        messages,
      ),
    [messages, runtimeZapsQuery.data],
  );
  const profilePubkeys = useMessageEventProfilePubkeys(
    timelineEvents,
    threadReplies,
    relaySelfPubkey,
  );
  const validationEvents = React.useMemo(
    () => [...timelineEvents, ...threadReplies],
    [threadReplies, timelineEvents],
  );
  const verifiedZaps = useVerifiedZapEvents(validationEvents);

  return [timelineEvents, profilePubkeys, verifiedZaps] as const;
}

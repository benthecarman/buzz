import * as React from "react";

import type { InboxContextMessage, InboxItem } from "@/features/home/lib/inbox";
import {
  getReactionTargetId,
  toInboxContextMessage,
} from "@/features/home/lib/inboxViewHelpers";
import { formatTimelineMessages } from "@/features/messages/lib/formatTimelineMessages";
import type { UserProfileLookup } from "@/features/profile/lib/identity";
import type { Channel, RelayEvent } from "@/shared/api/types";
import { KIND_REACTION } from "@/shared/constants/kinds";
import { useVerifiedZapEvents } from "@/features/wallet/lib/useVerifiedZapEvents";

type UseHomeInboxContextMessagesOptions = {
  channelMessages?: RelayEvent[];
  currentPubkey?: string;
  events: RelayEvent[];
  ownerProfiles?: UserProfileLookup;
  profiles?: UserProfileLookup;
  reactionEvents: RelayEvent[];
  relaySelfPubkey?: string | null;
  selectedChannel: Channel | null;
  selectedEventId: string | null;
  selectedItem: InboxItem | null;
  structuralEvents?: RelayEvent[];
};

export function useHomeInboxContextMessages({
  channelMessages,
  currentPubkey,
  events,
  ownerProfiles,
  profiles,
  reactionEvents,
  relaySelfPubkey,
  selectedChannel,
  selectedEventId,
  selectedItem,
  structuralEvents = [],
}: UseHomeInboxContextMessagesOptions): InboxContextMessage[] {
  const timelineEvents = React.useMemo(() => {
    if (!selectedItem) return [];
    const contextEventIds = new Set(events.map((event) => event.id));
    const contextReactions = [
      ...(channelMessages ?? []),
      ...reactionEvents,
    ].filter((event) => {
      if (event.kind !== KIND_REACTION) return false;
      const targetId = getReactionTargetId(event.tags);
      return Boolean(targetId && contextEventIds.has(targetId));
    });
    return [...events, ...structuralEvents, ...contextReactions];
  }, [channelMessages, events, reactionEvents, selectedItem, structuralEvents]);
  const verifiedZapEvents = useVerifiedZapEvents(timelineEvents);

  return React.useMemo(() => {
    if (!selectedItem) return [];

    const eventById = new Map(events.map((event) => [event.id, event]));
    const currentUserAvatarUrl = currentPubkey
      ? (profiles?.[currentPubkey.toLowerCase()]?.avatarUrl ?? null)
      : null;
    const timelineMessages = formatTimelineMessages(
      timelineEvents,
      selectedChannel,
      currentPubkey,
      currentUserAvatarUrl,
      profiles,
      undefined,
      undefined,
      undefined,
      relaySelfPubkey,
      ownerProfiles,
      verifiedZapEvents,
    );

    return timelineMessages.map((message) =>
      toInboxContextMessage(message, {
        eventById,
        fallbackAuthorPubkey: selectedItem.item.pubkey,
        profiles,
        selectedItemId: selectedEventId ?? selectedItem.id,
      }),
    );
  }, [
    currentPubkey,
    events,
    ownerProfiles,
    profiles,
    relaySelfPubkey,
    selectedChannel,
    selectedEventId,
    selectedItem,
    timelineEvents,
    verifiedZapEvents,
  ]);
}

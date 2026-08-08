import * as React from "react";

import { useAppShell } from "@/app/AppShellContext";
import { useHomeFeedQuery } from "@/features/home/hooks";
import { HomeView } from "@/features/home/ui/HomeView";
import {
  useZapHistory,
  zapHistoryFeedItems,
} from "@/features/wallet/lib/zapHistory";
import type { HomeFeedResponse } from "@/shared/api/types";
import {
  isRelayUnreachableError,
  RELAY_UNREACHABLE_MESSAGE,
} from "@/shared/lib/relayError";

type HomeScreenProps = {
  availableChannelIds: ReadonlySet<string>;
  currentPubkey?: string;
  onOpenContext: (
    channelId: string,
    messageId: string,
    threadRootId?: string | null,
  ) => void;
};

export function HomeScreen({
  availableChannelIds,
  currentPubkey,
  onOpenContext,
}: HomeScreenProps) {
  const homeFeedQuery = useHomeFeedQuery();
  const { threadActivityFeedItems } = useAppShell();
  const zapHistory = useZapHistory(currentPubkey);
  const zapFeedItems = React.useMemo(
    () => zapHistoryFeedItems(zapHistory),
    [zapHistory],
  );

  const augmentedFeed = React.useMemo((): HomeFeedResponse | undefined => {
    const extraActivity = [...threadActivityFeedItems, ...zapFeedItems];
    if (!homeFeedQuery.data && extraActivity.length === 0) return undefined;
    if (homeFeedQuery.data && extraActivity.length === 0) {
      return homeFeedQuery.data;
    }

    const base = homeFeedQuery.data ?? {
      feed: {
        mentions: [],
        needsAction: [],
        activity: [],
        agentActivity: [],
      },
      meta: { since: 0, total: 0, generatedAt: 0 },
    };
    const existingActivityIds = new Set(
      base.feed.activity.map((item) => item.id),
    );

    return {
      ...base,
      feed: {
        ...base.feed,
        activity: [
          ...base.feed.activity,
          ...extraActivity.filter((item) => !existingActivityIds.has(item.id)),
        ],
      },
    };
  }, [homeFeedQuery.data, threadActivityFeedItems, zapFeedItems]);

  return (
    <div className="flex min-h-0 min-w-0 flex-1 flex-col overflow-hidden">
      <HomeView
        availableChannelIds={availableChannelIds}
        currentPubkey={currentPubkey}
        errorMessage={
          homeFeedQuery.error !== null && homeFeedQuery.error !== undefined
            ? isRelayUnreachableError(homeFeedQuery.error)
              ? RELAY_UNREACHABLE_MESSAGE
              : homeFeedQuery.error instanceof Error
                ? homeFeedQuery.error.message
                : undefined
            : undefined
        }
        feed={augmentedFeed}
        isLoading={homeFeedQuery.isLoading}
        onOpenContext={onOpenContext}
        onRefresh={() => {
          void homeFeedQuery.refetch();
        }}
      />
    </div>
  );
}

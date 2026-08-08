import { Zap } from "lucide-react";

import { useCommunities } from "@/features/communities/useCommunities";
import { useIdentityQuery } from "@/shared/api/hooks";
import { formatBitcoin } from "../lib/formatBitcoin";
import { useZapHistory } from "../lib/zapHistory";

const DISPLAY_LIMIT = 100;
const timestampFormatter = new Intl.DateTimeFormat(undefined, {
  dateStyle: "medium",
  timeStyle: "short",
});

export function ZapHistoryCard() {
  const ownerPubkey = useIdentityQuery().data?.pubkey;
  const relayUrl = useCommunities().activeCommunity?.relayUrl;
  const history = useZapHistory(ownerPubkey, relayUrl);
  const visibleHistory = history.slice(0, DISPLAY_LIMIT);

  return (
    <div
      className="rounded-2xl border border-border/70 bg-background p-5"
      data-testid="wallet-zap-history"
    >
      <div className="flex items-start justify-between gap-3">
        <div>
          <h3 className="text-sm font-semibold">Zap history</h3>
          <p className="mt-1 text-sm text-muted-foreground">
            Zaps received by you and your managed agents.
          </p>
        </div>
        <Zap className="h-5 w-5 text-amber-500" />
      </div>

      {visibleHistory.length === 0 ? (
        <p className="mt-4 text-sm text-muted-foreground">
          No zaps received yet.
        </p>
      ) : (
        <div className="mt-4 max-h-96 divide-y divide-border/60 overflow-y-auto">
          {visibleHistory.map((item) => (
            <div className="flex items-start gap-3 py-3" key={item.eventId}>
              <div className="flex h-8 w-8 shrink-0 items-center justify-center rounded-full bg-amber-500/10 text-amber-600 dark:text-amber-300">
                <Zap className="h-4 w-4" />
              </div>
              <div className="min-w-0 flex-1">
                <div className="flex items-baseline justify-between gap-3">
                  <p className="truncate text-sm font-medium">
                    {item.recipientName}
                  </p>
                  <p className="shrink-0 text-sm font-semibold">
                    {formatBitcoin(item.amount)}
                  </p>
                </div>
                {item.comment.trim() ? (
                  <p className="mt-0.5 truncate text-sm text-muted-foreground">
                    {item.comment.trim()}
                  </p>
                ) : null}
                <p className="mt-0.5 text-xs text-muted-foreground">
                  {timestampFormatter.format(new Date(item.createdAt * 1_000))}
                </p>
              </div>
            </div>
          ))}
        </div>
      )}

      {history.length > DISPLAY_LIMIT ? (
        <p className="mt-3 text-xs text-muted-foreground">
          Showing the latest {DISPLAY_LIMIT} of {history.length} zaps.
        </p>
      ) : null}
    </div>
  );
}

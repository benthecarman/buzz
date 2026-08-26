import * as React from "react";

import type { WalletVerifiedZapEvent } from "@/features/wallet/types";
import type { RelayEvent } from "@/shared/api/types";
import { KIND_BOLT12_ZAP } from "@/shared/constants/kinds";
import {
  useStableArrayShallow,
  useStableMap,
} from "@/shared/hooks/useStableReference";
import { parseRelayZapEvent } from "./relayZap";

const EMPTY_VERIFIED_ZAPS = new Map<string, WalletVerifiedZapEvent>();

/** Extract display data from zap events that the relay already validated. */
export function useVerifiedZapEvents(
  events: readonly RelayEvent[],
): ReadonlyMap<string, WalletVerifiedZapEvent> {
  const zapEvents = useStableArrayShallow(
    React.useMemo(
      () => events.filter((event) => event.kind === KIND_BOLT12_ZAP),
      [events],
    ),
  );
  const parsed = React.useMemo(() => {
    if (zapEvents.length === 0) return EMPTY_VERIFIED_ZAPS;
    const zaps = new Map<string, WalletVerifiedZapEvent>();
    for (const event of zapEvents) {
      const zap = parseRelayZapEvent(event);
      if (zap) zaps.set(event.id, zap);
    }
    return zaps;
  }, [zapEvents]);
  return useStableMap(parsed);
}

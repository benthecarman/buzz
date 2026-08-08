import * as React from "react";

import { parseWalletZapEvents } from "@/features/wallet/api";
import type { WalletVerifiedZapEvent } from "@/features/wallet/types";
import type { RelayEvent } from "@/shared/api/types";
import { KIND_BOLT12_ZAP } from "@/shared/constants/kinds";
import { useStableArrayShallow } from "@/shared/hooks/useStableReference";

const EMPTY_VERIFIED_ZAPS: ReadonlyMap<string, WalletVerifiedZapEvent> =
  new Map();

/** Parse relay zap events natively before exposing them to timeline formatters. */
export function useVerifiedZapEvents(
  events: readonly RelayEvent[],
): ReadonlyMap<string, WalletVerifiedZapEvent> {
  const zapEvents = useStableArrayShallow(
    React.useMemo(
      () => events.filter((event) => event.kind === KIND_BOLT12_ZAP),
      [events],
    ),
  );
  const zapKey = React.useMemo(
    () =>
      zapEvents
        .map((event) => event.id)
        .sort()
        .join(","),
    [zapEvents],
  );
  const [result, setResult] = React.useState<{
    key: string;
    zaps: ReadonlyMap<string, WalletVerifiedZapEvent>;
  }>({ key: "", zaps: EMPTY_VERIFIED_ZAPS });

  React.useEffect(() => {
    if (zapEvents.length === 0) {
      setResult((current) =>
        current.key === zapKey && current.zaps === EMPTY_VERIFIED_ZAPS
          ? current
          : { key: zapKey, zaps: EMPTY_VERIFIED_ZAPS },
      );
      return;
    }
    let cancelled = false;
    void parseWalletZapEvents(zapEvents)
      .then((zaps) => {
        if (cancelled) return;
        setResult({
          key: zapKey,
          zaps: new Map(zaps.map((zap) => [zap.eventId, zap])),
        });
      })
      .catch((error) => {
        if (cancelled) return;
        console.error("Failed to validate BOLT12 zap events", error);
        setResult({ key: zapKey, zaps: EMPTY_VERIFIED_ZAPS });
      });
    return () => {
      cancelled = true;
    };
  }, [zapEvents, zapKey]);

  return result.key === zapKey ? result.zaps : EMPTY_VERIFIED_ZAPS;
}

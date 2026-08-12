import { listen } from "@tauri-apps/api/event";
import { useEffect, useState } from "react";
import { toast } from "sonner";

import { useFeatureEnabled } from "@/shared/features";
import { setWalletPollingEnabled } from "./api";
import { INCOMING_WALLET_PAYMENT_EVENT } from "./events";
import { useBitcoinCompileEnabled } from "./hooks";
import { formatBitcoin } from "./lib/formatBitcoin";
import type { WalletIncomingPaymentEvent } from "./types";

/** Keep ordinary wallet receives live independently of the wallet panel. */
export function useIncomingWalletPayments(): void {
  const compiled = useBitcoinCompileEnabled();
  const enabled = useFeatureEnabled("bitcoin");
  const [listenerAttempted, setListenerAttempted] = useState(false);

  useEffect(() => {
    if (!compiled) {
      setListenerAttempted(false);
      return;
    }
    let disposed = false;
    let unlisten: (() => void) | null = null;
    void (async () => {
      try {
        const nextUnlisten = await listen<WalletIncomingPaymentEvent>(
          INCOMING_WALLET_PAYMENT_EVENT,
          ({ payload }) => {
            toast.success("Bitcoin received", {
              description: formatBitcoin(payload.transaction.amount),
            });
          },
        );
        if (disposed) {
          nextUnlisten();
          return;
        }
        unlisten = nextUnlisten;
      } catch (error) {
        console.error("Failed to listen for wallet payments", error);
      }
      if (!disposed) setListenerAttempted(true);
    })();
    return () => {
      disposed = true;
      unlisten?.();
    };
  }, [compiled]);

  useEffect(() => {
    if (!compiled || !listenerAttempted) return;
    void setWalletPollingEnabled(enabled).catch((error) => {
      console.error("Failed to apply wallet payment polling setting", error);
    });
  }, [compiled, enabled, listenerAttempted]);
}

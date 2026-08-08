import { listen } from "@tauri-apps/api/event";
import { useEffect } from "react";
import { toast } from "sonner";

import { useFeatureEnabled } from "@/shared/features";
import { setWalletPollingEnabled } from "./api";
import { useBitcoinCompileEnabled } from "./hooks";
import { formatBitcoin } from "./lib/formatBitcoin";
import type { WalletTransaction } from "./types";

const INCOMING_PAYMENT_EVENT = "wallet-incoming-payment";

/** Keep ordinary wallet receives live independently of the wallet panel. */
export function useIncomingWalletPayments(): void {
  const compiled = useBitcoinCompileEnabled();
  const enabled = useFeatureEnabled("bitcoin");

  useEffect(() => {
    if (!compiled) return;
    let disposed = false;
    let unlisten: (() => void) | null = null;
    void listen<WalletTransaction>(INCOMING_PAYMENT_EVENT, ({ payload }) => {
      toast.success("Bitcoin received", {
        description: formatBitcoin(payload.amount),
      });
    }).then((nextUnlisten) => {
      if (disposed) nextUnlisten();
      else unlisten = nextUnlisten;
    });
    return () => {
      disposed = true;
      unlisten?.();
    };
  }, [compiled]);

  useEffect(() => {
    if (!compiled) return;
    void setWalletPollingEnabled(enabled).catch((error) => {
      console.error("Failed to apply wallet payment polling setting", error);
    });
  }, [compiled, enabled]);
}

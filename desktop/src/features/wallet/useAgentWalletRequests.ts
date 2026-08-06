import * as React from "react";
import { toast } from "sonner";

import { relayClient } from "@/shared/api/relayClient";
import type { RelayEvent } from "@/shared/api/types";
import { KIND_NWC_REQUEST } from "@/shared/constants/kinds";
import { useFeatureEnabled } from "@/shared/features";
import { truncatePubkey } from "@/shared/lib/pubkey";
import { formatBitcoin } from "./lib/formatBitcoin";
import {
  buildNwcWalletResponse,
  parseNwcWalletRequest,
  sendWalletPayment,
} from "./api";

const RESPONSE_TIMEOUT = "Timed out returning the wallet response";
const RESPONSE_FAILED = "Failed to return the wallet response";

/** Act as the user-wallet NWC service for requests from managed agents. */
export function useAgentWalletRequests(ownerPubkey: string | undefined) {
  const walletEnabled = useFeatureEnabled("bitcoin");
  const handledRef = React.useRef(new Set<string>());

  const respondWithError = React.useEffectEvent(
    async (event: RelayEvent, code: string, message: string) => {
      const response = await buildNwcWalletResponse({
        event,
        errorCode: code,
        errorMessage: message,
      });
      await relayClient.publishEvent(
        response,
        RESPONSE_TIMEOUT,
        RESPONSE_FAILED,
      );
    },
  );

  const handleRequest = React.useEffectEvent(async (event: RelayEvent) => {
    if (handledRef.current.has(event.id)) return;
    let request: Awaited<ReturnType<typeof parseNwcWalletRequest>>;
    try {
      request = await parseNwcWalletRequest(event);
    } catch (error) {
      console.error("Rejected invalid agent NWC request", error);
      return;
    }
    handledRef.current.add(event.id);
    const recipient = truncatePubkey(request.recipientPubkey);
    const description = request.comment.trim()
      ? `${formatBitcoin(request.amount)} to ${recipient} · ${request.comment.trim().slice(0, 120)}`
      : `${formatBitcoin(request.amount)} to ${recipient}`;

    toast(`${request.agentName} requests a zap`, {
      description,
      duration: Number.POSITIVE_INFINITY,
      action: {
        label: "Approve",
        onClick: () => {
          void (async () => {
            let payment: Awaited<ReturnType<typeof sendWalletPayment>>;
            try {
              payment = await sendWalletPayment({
                destination: request.destination,
                amount: request.amount,
                message: request.payerNote,
                requestId: request.requestId,
              });
            } catch (error) {
              const message =
                error instanceof Error ? error.message : "The payment failed";
              try {
                await respondWithError(event, "PAYMENT_FAILED", message);
              } catch (responseError) {
                console.error(
                  "Failed to return NWC payment error",
                  responseError,
                );
              }
              toast.error("Agent zap failed", { description: message });
              return;
            }
            try {
              const response = await buildNwcWalletResponse({ event, payment });
              await relayClient.publishEvent(
                response,
                RESPONSE_TIMEOUT,
                RESPONSE_FAILED,
              );
              toast.success(`${request.agentName}'s zap was paid`, {
                description: formatBitcoin(request.amount),
              });
            } catch (error) {
              console.error("Payment succeeded but NWC response failed", error);
              toast.warning("Zap paid; response delivery failed", {
                description: `${request.agentName} may need to check the payment status.`,
              });
            }
          })();
        },
      },
      cancel: {
        label: "Deny",
        onClick: () => {
          void respondWithError(
            event,
            "RESTRICTED",
            "The wallet owner denied this payment",
          ).catch((error) =>
            console.error("Failed to return NWC denial", error),
          );
        },
      },
    });
  });

  React.useEffect(() => {
    const owner = ownerPubkey?.trim().toLowerCase();
    if (!walletEnabled || !owner) return;
    let cancelled = false;
    let dispose: (() => Promise<void>) | undefined;
    void relayClient
      .subscribeLive(
        {
          kinds: [KIND_NWC_REQUEST],
          "#p": [owner],
          since: Math.floor(Date.now() / 1_000) - 5,
          limit: 0,
        },
        (event) => void handleRequest(event),
      )
      .then((nextDispose) => {
        if (cancelled) void nextDispose();
        else dispose = nextDispose;
      })
      .catch((error) =>
        console.error("Failed to subscribe for agent wallet requests", error),
      );
    return () => {
      cancelled = true;
      handledRef.current.clear();
      void dispose?.();
    };
  }, [ownerPubkey, walletEnabled]);
}

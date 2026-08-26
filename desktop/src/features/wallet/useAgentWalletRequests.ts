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
  handleNwcWalletRequest,
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
    let handling: Awaited<ReturnType<typeof handleNwcWalletRequest>>;
    try {
      handling = await handleNwcWalletRequest(event);
    } catch (error) {
      console.error("Rejected invalid agent NWC request", error);
      return;
    }
    handledRef.current.add(event.id);
    if (handling.action !== "approval_required" && handling.response) {
      try {
        await relayClient.publishEvent(
          handling.response,
          RESPONSE_TIMEOUT,
          RESPONSE_FAILED,
        );
        if (handling.request && handling.action === "payment_completed") {
          toast.success(`${handling.request.agentName}'s payment was paid`, {
            description: `${formatBitcoin(handling.request.amount)} · Auto-approved within budget`,
          });
        } else if (handling.request && handling.action === "payment_pending") {
          toast.warning(`${handling.request.agentName}'s payment is pending`, {
            description:
              "The reserved budget remains in use while the wallet reconciles.",
          });
        } else if (handling.request && handling.action === "payment_failed") {
          toast.error(`${handling.request.agentName}'s payment failed`);
        }
      } catch (error) {
        console.error("Failed to return automatic NWC response", error);
      }
      return;
    }
    const request = handling.request;
    if (!request) return;
    const isZap = request.requestType === "zap";
    const recipient = request.recipientPubkey
      ? truncatePubkey(request.recipientPubkey)
      : "BIP-321 payment request";
    const description = request.comment.trim()
      ? `${formatBitcoin(request.amount)} to ${recipient} · ${request.comment.trim().slice(0, 120)}`
      : `${formatBitcoin(request.amount)} to ${recipient}`;
    const approvalDuration = request.expiresAtMs - Date.now();
    if (approvalDuration <= 0) return;
    const toastId = `wallet-request-${event.id}`;

    toast(`${request.agentName} requests ${isZap ? "a zap" : "a payment"}`, {
      id: toastId,
      description,
      duration: approvalDuration,
      action: {
        label: "Approve",
        onClick: () => {
          void (async () => {
            if (Date.now() >= request.expiresAtMs) {
              toast.dismiss(toastId);
              return;
            }
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
              toast.error(`Agent ${isZap ? "zap" : "payment"} failed`, {
                description: message,
              });
              return;
            }
            try {
              const response = await buildNwcWalletResponse({ event, payment });
              await relayClient.publishEvent(
                response,
                RESPONSE_TIMEOUT,
                RESPONSE_FAILED,
              );
              toast.success(
                `${request.agentName}'s ${isZap ? "zap" : "payment"} was paid`,
                {
                  description: formatBitcoin(request.amount),
                },
              );
            } catch (error) {
              console.error("Payment succeeded but NWC response failed", error);
              toast.warning(
                `${isZap ? "Zap" : "Payment"} paid; response delivery failed`,
                {
                  description: `${request.agentName} may need to check the payment status.`,
                },
              );
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

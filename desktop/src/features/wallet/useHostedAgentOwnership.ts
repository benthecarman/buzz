import * as React from "react";

import { relayClient } from "@/shared/api/relayClient";
import { buildHostedAgentOwnerAttestation } from "@/shared/api/tauri";
import type { Channel, RelayEvent } from "@/shared/api/types";
import {
  KIND_BOLT12_ZAP,
  KIND_HOSTED_AGENT_PLAN,
  KIND_STREAM_MESSAGE_V2,
} from "@/shared/constants/kinds";
import {
  catchUpHostedAgentOwnershipRequests,
  hostedAgentDmChannelKey,
  isHostedAgentOwnershipRequest,
  matchesHostedAgentPurchase,
  ownershipClaimLeaseId,
  ownershipClaimRequestReference,
} from "./hostedAgentOwnership";

/** Claim first ownership when a factory request targets the signed-in buyer. */
export function useHostedAgentOwnership(
  buyerPubkey: string | undefined,
  channels: Channel[],
): void {
  const handled = React.useRef(new Set<string>());
  const dmChannelKey = hostedAgentDmChannelKey(channels);

  const accept = React.useEffectEvent(async (request: RelayEvent) => {
    if (!buyerPubkey || handled.current.has(request.id)) return;
    handled.current.add(request.id);
    try {
      const leaseId = ownershipClaimLeaseId(request);
      if (!leaseId) throw new Error("Ownership request has no lease ID.");
      const existing = await relayClient.fetchFirstEvent({
        authors: [buyerPubkey],
        kinds: [KIND_STREAM_MESSAGE_V2],
        "#d": [leaseId],
        limit: 1,
      });
      if (existing?.content.trim() === "") return;
      const zapId = ownershipClaimRequestReference(request, "zap");
      const planId = ownershipClaimRequestReference(request, "plan");
      if (!zapId) throw new Error("Ownership request has no zap reference.");
      if (!planId) throw new Error("Ownership request has no plan reference.");
      const [zap, plan] = await Promise.all([
        relayClient.fetchFirstEvent({
          ids: [zapId],
          kinds: [KIND_BOLT12_ZAP],
          limit: 1,
        }),
        relayClient.fetchFirstEvent({
          ids: [planId],
          kinds: [KIND_HOSTED_AGENT_PLAN],
          limit: 1,
        }),
      ]);
      if (!zap || !matchesHostedAgentPurchase(request, zap, buyerPubkey)) {
        throw new Error("Ownership request does not match a buyer zap.");
      }
      if (!plan) throw new Error("Hosted-agent plan is not available.");
      const attestation = await buildHostedAgentOwnerAttestation(
        request,
        zap,
        plan,
      );
      await relayClient.publishEvent(
        attestation,
        "Timed out publishing the hosted-agent owner attestation.",
        "Failed to publish the hosted-agent owner attestation.",
      );
    } catch (error) {
      handled.current.delete(request.id);
      console.error("Failed to accept hosted-agent ownership", error);
    }
  });

  React.useEffect(() => {
    if (!buyerPubkey) return;
    let disposed = false;
    void catchUpHostedAgentOwnershipRequests({
      buyerPubkey,
      fetchEvents: (filter) => relayClient.fetchEvents(filter),
      onRequest: async (request) => {
        if (!disposed) await accept(request);
      },
    }).catch((error) => {
      console.error(
        "Failed to catch up hosted-agent ownership requests",
        error,
      );
    });
    return () => {
      disposed = true;
    };
  }, [buyerPubkey]);

  React.useEffect(() => {
    if (!buyerPubkey) return;
    let disposed = false;
    const cleanups: Array<() => Promise<void>> = [];
    const dmChannelIds = dmChannelKey ? dmChannelKey.split(",") : [];

    void Promise.all(
      dmChannelIds.map(async (channelId) => {
        const unsubscribe = await relayClient.subscribeLive(
          {
            kinds: [KIND_STREAM_MESSAGE_V2],
            "#h": [channelId],
            "#p": [buyerPubkey],
            limit: 100,
          },
          (event) => {
            if (isHostedAgentOwnershipRequest(event, buyerPubkey)) {
              void accept(event);
            }
          },
        );
        if (disposed) {
          await unsubscribe();
        } else {
          cleanups.push(unsubscribe);
        }
      }),
    ).catch((error) => {
      console.error("Failed to watch hosted-agent ownership requests", error);
    });

    return () => {
      disposed = true;
      for (const cleanup of cleanups) void cleanup();
    };
  }, [buyerPubkey, dmChannelKey]);
}

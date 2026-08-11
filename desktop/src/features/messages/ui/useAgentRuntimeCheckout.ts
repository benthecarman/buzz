import * as React from "react";
import { useRelayAgentsQuery } from "@/features/agents/hooks";
import { useCommunities } from "@/features/communities/useCommunities";
import {
  activeAccessZap,
  getAgentRuntimeStatus,
  runtimeZapMessageTag,
} from "@/features/agents/runtimePayments";
import { sendAgentRuntimeZap } from "@/features/wallet/api";
import { walletCommandError } from "@/features/wallet/lib/walletError";
import type { ChannelType, ManagedAgent } from "@/shared/api/types";
import { useIdentityQuery } from "@/shared/api/hooks";
import { normalizePubkey } from "@/shared/lib/pubkey";
import {
  activeStoredRuntimeZap,
  loadAgentRuntimeCheckout,
  saveAgentRuntimeCheckout,
} from "../lib/agentRuntimeCheckoutStorage";
import type { RuntimeCheckoutRow } from "./AgentRuntimeCheckoutDialog";
import { getErrorMessage } from "./useMentionSendFlow.helpers";

type PendingRow = RuntimeCheckoutRow & {
  zapIdempotencyKey: string;
  zapEventId: string | null;
  validUntilSeconds: number | null;
};

type PendingCheckout = { channelId: string; rows: PendingRow[] };

export function useAgentRuntimeCheckout(channelType: ChannelType | null) {
  const { activeCommunity } = useCommunities();
  const [checkout, setCheckout] = React.useState<PendingCheckout | null>(null);
  const [error, setError] = React.useState<string | null>(null);
  const [isPaying, setIsPaying] = React.useState(false);
  const resolveRef = React.useRef<((tags: string[][] | null) => void) | null>(
    null,
  );
  const relayAgentsQuery = useRelayAgentsQuery();
  const identityQuery = useIdentityQuery();
  const checkoutScope = React.useCallback(
    (channelId: string) => `${activeCommunity?.id ?? "none"}:${channelId}`,
    [activeCommunity?.id],
  );

  const beginRuntimeCheckout = React.useCallback(
    async (
      agentPubkeys: string[],
      channelId: string,
      managedAgentsByPubkey: Map<string, ManagedAgent>,
    ): Promise<string[][] | null> => {
      const relayAgents =
        relayAgentsQuery.data ?? (await relayAgentsQuery.refetch()).data ?? [];
      const wanted = new Set(agentPubkeys.map(normalizePubkey));
      const payerPubkey = identityQuery.data?.pubkey
        ? normalizePubkey(identityQuery.data.pubkey)
        : "";
      const paidAgents = relayAgents
        .filter((agent) => wanted.has(normalizePubkey(agent.pubkey)))
        .filter(
          (agent) => !managedAgentsByPubkey.has(normalizePubkey(agent.pubkey)),
        )
        .filter(
          (agent) =>
            !agent.ownerPubkey ||
            normalizePubkey(agent.ownerPubkey) !== payerPubkey,
        )
        .filter(
          (agent): agent is typeof agent & { pricePerMinuteSats: number } =>
            agent.pricePerMinuteSats != null && agent.pricePerMinuteSats > 0,
        );
      if (paidAgents.length === 0) return [];
      if (channelType === "dm") {
        throw new Error("Paid Agent access is unavailable in direct messages.");
      }
      const statuses = await Promise.all(
        paidAgents.map((agent) =>
          getAgentRuntimeStatus({ agentPubkey: agent.pubkey, channelId }),
        ),
      );
      const storageScope = checkoutScope(channelId);
      const stored = loadAgentRuntimeCheckout(storageScope);
      const sameAgents =
        stored?.channelId === channelId &&
        stored.rows.length === paidAgents.length &&
        stored.rows.every(
          (row, index) =>
            row.pubkey === normalizePubkey(paidAgents[index]?.pubkey ?? ""),
        );
      const rows = paidAgents.map((agent, index): PendingRow => {
        const status = statuses[index];
        const pricing = status?.pricing;
        const activeZap = activeAccessZap(status);
        const previous = sameAgents ? stored?.rows[index] : undefined;
        const cachedZap = activeStoredRuntimeZap(
          previous,
          Math.floor(Date.now() / 1_000),
        );
        const zapEventId = activeZap?.zapEventId ?? cachedZap;
        return {
          pubkey: normalizePubkey(agent.pubkey),
          name: agent.name,
          ownerPubkey: agent.ownerPubkey ?? null,
          priceSats: pricing?.priceSats ?? agent.pricePerMinuteSats,
          invocationWindowSeconds: pricing?.invocationWindowSeconds ?? 300,
          pricingEventJson: pricing?.pricingEventJson ?? null,
          needsPayment: zapEventId === null,
          zapIdempotencyKey: previous?.zapIdempotencyKey ?? crypto.randomUUID(),
          zapEventId,
          validUntilSeconds: activeZap
            ? activeZap.validUntil
            : cachedZap
              ? (previous?.validUntilSeconds ?? null)
              : null,
        };
      });
      if (rows.every((row) => row.zapEventId !== null)) {
        saveAgentRuntimeCheckout(storageScope, { channelId, rows });
        return rows.map((row) =>
          runtimeZapMessageTag(row.pubkey, row.zapEventId as string),
        );
      }
      const nextCheckout = { channelId, rows };
      saveAgentRuntimeCheckout(storageScope, nextCheckout);
      setError(null);
      setCheckout(nextCheckout);
      return new Promise<string[][] | null>((resolve) => {
        resolveRef.current = resolve;
      });
    },
    [
      channelType,
      checkoutScope,
      identityQuery.data?.pubkey,
      relayAgentsQuery.data,
      relayAgentsQuery.refetch,
    ],
  );

  const onDismiss = React.useCallback(() => {
    if (isPaying) return;
    resolveRef.current?.(null);
    resolveRef.current = null;
    setCheckout(null);
    setError(null);
  }, [isPaying]);

  const onConfirm = React.useCallback(() => {
    if (!checkout || isPaying) return;
    setIsPaying(true);
    setError(null);
    const storageScope = checkoutScope(checkout.channelId);
    void completeCheckout(checkout, (rows) => {
      const nextCheckout = { ...checkout, rows };
      saveAgentRuntimeCheckout(storageScope, nextCheckout);
      setCheckout(nextCheckout);
    })
      .then((tags) => {
        resolveRef.current?.(tags);
        resolveRef.current = null;
        setCheckout(null);
      })
      .catch((cause) => {
        setError(getErrorMessage(cause, "Could not buy Agent access."));
      })
      .finally(() => setIsPaying(false));
  }, [checkout, checkoutScope, isPaying]);

  return {
    beginRuntimeCheckout,
    runtimeCheckoutProps: {
      error,
      isPaying,
      onConfirm,
      onDismiss,
      open: checkout !== null,
      rows: checkout?.rows ?? [],
    },
  };
}

async function completeCheckout(
  checkout: PendingCheckout,
  updateRows: (rows: PendingRow[]) => void,
): Promise<string[][]> {
  const rows = [...checkout.rows];
  for (let index = 0; index < rows.length; index += 1) {
    let row = rows[index];
    if (!row || row.zapEventId) continue;
    if (!row.pricingEventJson) {
      throw new Error(`${row.name} does not currently accept payment.`);
    }
    try {
      const result = await sendAgentRuntimeZap({
        agentPubkey: row.pubkey,
        channelId: checkout.channelId,
        pricingEventJson: row.pricingEventJson,
        idempotencyKey: row.zapIdempotencyKey,
      });
      if (!result.proofPublished || !result.proofEventId) {
        throw new Error(
          `${row.name} received the payment without a zap proof.`,
        );
      }
      row = {
        ...row,
        zapEventId: result.proofEventId,
        validUntilSeconds:
          Math.floor(Date.now() / 1_000) + row.invocationWindowSeconds,
      };
      row = { ...row, needsPayment: false };
      rows[index] = row;
      updateRows([...rows]);
    } catch (cause) {
      if (walletCommandError(cause).code === "payment_failed") {
        row = { ...row, zapIdempotencyKey: crypto.randomUUID() };
        rows[index] = row;
        updateRows([...rows]);
      }
      throw cause;
    }
  }
  return rows.map((row) => {
    if (!row.zapEventId) {
      throw new Error(`${row.name} has no settled access zap.`);
    }
    return runtimeZapMessageTag(row.pubkey, row.zapEventId);
  });
}

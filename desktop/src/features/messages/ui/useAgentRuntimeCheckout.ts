import * as React from "react";
import { useRelayAgentsQuery } from "@/features/agents/hooks";
import { useCommunities } from "@/features/communities/useCommunities";
import {
  type AgentRuntimeCapMinutes,
  getAgentRuntimeBalance,
  requestAgentRuntimeReservation,
  runtimeReservationMessageTag,
} from "@/features/agents/runtimePayments";
import { sendAgentRuntimeZap } from "@/features/wallet/api";
import { walletCommandError } from "@/features/wallet/lib/walletError";
import type { ChannelType, ManagedAgent } from "@/shared/api/types";
import { useIdentityQuery } from "@/shared/api/hooks";
import { normalizePubkey } from "@/shared/lib/pubkey";
import {
  clearAgentRuntimeCheckout,
  loadAgentRuntimeCheckout,
  saveAgentRuntimeCheckout,
} from "../lib/agentRuntimeCheckoutStorage";
import type { RuntimeCheckoutRow } from "./AgentRuntimeCheckoutDialog";
import { getErrorMessage } from "./useMentionSendFlow.helpers";

type PendingRow = RuntimeCheckoutRow & {
  requestId: string;
  zapIdempotencyKey: string;
  quoteEventJson: string | null;
  paymentSent: boolean;
  reservationTag: string[] | null;
  restored: boolean;
};

type PendingCheckout = { channelId: string; rows: PendingRow[] };

export function useAgentRuntimeCheckout(channelType: ChannelType | null) {
  const { activeCommunity } = useCommunities();
  const [checkout, setCheckout] = React.useState<PendingCheckout | null>(null);
  const [capMinutes, setCapMinutes] =
    React.useState<AgentRuntimeCapMinutes>(15);
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
        throw new Error("Paid Agent invocation is unavailable in DMs.");
      }
      const balances = await Promise.all(
        paidAgents.map((agent) =>
          getAgentRuntimeBalance({ agentPubkey: agent.pubkey, channelId }),
        ),
      );
      let rows = paidAgents
        .map(
          (agent, index): PendingRow => ({
            pubkey: normalizePubkey(agent.pubkey),
            name: agent.name,
            rateSats: agent.pricePerMinuteSats,
            availableMs: balances[index]?.availableMs ?? 0,
            requestId: crypto.randomUUID(),
            zapIdempotencyKey: crypto.randomUUID(),
            quoteEventJson: null,
            paymentSent: false,
            reservationTag: null,
            restored: false,
          }),
        )
        .sort((left, right) => left.pubkey.localeCompare(right.pubkey));
      const storageScope = checkoutScope(channelId);
      const stored = loadAgentRuntimeCheckout(storageScope);
      const sameAgents =
        stored?.channelId === channelId &&
        stored?.rows.length === rows.length &&
        stored.rows.every((row, index) => row.pubkey === rows[index]?.pubkey);
      if (stored && sameAgents) {
        setCapMinutes(stored.capMinutes);
        rows = rows.map((row, index) => ({
          ...row,
          requestId: stored.rows[index]?.requestId ?? row.requestId,
          zapIdempotencyKey:
            stored.rows[index]?.zapIdempotencyKey ?? row.zapIdempotencyKey,
          quoteEventJson: stored.rows[index]?.quoteEventJson ?? null,
          paymentSent: stored.rows[index]?.paymentSent ?? false,
          // Re-query the same request id after a renderer restart. The Agent
          // will return the exact open reservation, or reject it if its
          // admission deadline already closed.
          reservationTag: null,
          restored: true,
        }));
      } else if (stored) {
        clearAgentRuntimeCheckout(storageScope);
      }
      setError(null);
      const nextCheckout = { channelId, rows };
      saveAgentRuntimeCheckout(storageScope, {
        ...nextCheckout,
        capMinutes: stored && sameAgents ? stored.capMinutes : capMinutes,
      });
      setCheckout(nextCheckout);
      return new Promise<string[][] | null>((resolve) => {
        resolveRef.current = resolve;
      });
    },
    [
      capMinutes,
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
    saveAgentRuntimeCheckout(storageScope, { ...checkout, capMinutes });
    void completeCheckout(checkout, capMinutes, (rows) => {
      const nextCheckout = { ...checkout, rows };
      saveAgentRuntimeCheckout(storageScope, { ...nextCheckout, capMinutes });
      setCheckout(nextCheckout);
    })
      .then((tags) => {
        resolveRef.current?.(tags);
        resolveRef.current = null;
        setCheckout(null);
      })
      .catch((cause) => {
        setError(getErrorMessage(cause, "Could not reserve Agent runtime."));
      })
      .finally(() => setIsPaying(false));
  }, [capMinutes, checkout, checkoutScope, isPaying]);

  const capLocked =
    checkout?.rows.some(
      (row) => row.paymentSent || row.reservationTag !== null,
    ) ?? false;
  return {
    beginRuntimeCheckout,
    markRuntimeCheckoutSent: (channelId: string) =>
      clearAgentRuntimeCheckout(checkoutScope(channelId)),
    runtimeCheckoutProps: {
      capMinutes,
      capLocked,
      error,
      isPaying,
      onCapChange: setCapMinutes,
      onConfirm,
      onDismiss,
      open: checkout !== null,
      rows: checkout?.rows ?? [],
    },
  };
}

async function completeCheckout(
  checkout: PendingCheckout,
  capMinutes: AgentRuntimeCapMinutes,
  updateRows: (rows: PendingRow[]) => void,
): Promise<string[][]> {
  const rows = [...checkout.rows];
  const initial = await Promise.all(
    rows.map((row) =>
      row.reservationTag
        ? null
        : requestAgentRuntimeReservation({
            agentPubkey: row.pubkey,
            channelId: checkout.channelId,
            capMinutes,
            requestId: row.requestId,
          }),
    ),
  );
  for (let index = 0; index < rows.length; index += 1) {
    let row = rows[index];
    if (!row || row.reservationTag) continue;
    let result = initial[index];
    if (!result) continue;
    if (result.response.status === "unavailable" && row.restored) {
      if (row.quoteEventJson && !row.paymentSent) {
        try {
          await sendAgentRuntimeZap({
            quoteEventJson: row.quoteEventJson,
            idempotencyKey: row.zapIdempotencyKey,
          });
          row = { ...row, paymentSent: true };
          rows[index] = row;
          updateRows([...rows]);
        } catch (cause) {
          const code = walletCommandError(cause).code;
          if (code !== "runtime_quote_expired" && code !== "payment_failed") {
            throw cause;
          }
          // An existing wallet attempt may replay after quote expiry. This
          // error therefore proves no attempt was created for the old key.
          row = {
            ...row,
            zapIdempotencyKey: crypto.randomUUID(),
            quoteEventJson: null,
          };
          rows[index] = row;
          updateRows([...rows]);
        }
      }
      row = {
        ...row,
        requestId: crypto.randomUUID(),
        reservationTag: null,
        restored: false,
      };
      rows[index] = row;
      updateRows([...rows]);
      result = await requestAgentRuntimeReservation({
        agentPubkey: row.pubkey,
        channelId: checkout.channelId,
        capMinutes,
        requestId: row.requestId,
      });
    }
    if (result.response.status === "unavailable") {
      throw new Error(`${row.name} is unavailable for paid runtime.`);
    }
    if (result.response.status === "payment_required" && !row.paymentSent) {
      if (
        row.quoteEventJson &&
        row.quoteEventJson !== result.responseEventJson
      ) {
        throw new Error(
          `${row.name} returned conflicting terms for the same runtime request.`,
        );
      }
      const quoteEventJson = row.quoteEventJson ?? result.responseEventJson;
      row = {
        ...row,
        quoteEventJson,
      };
      rows[index] = row;
      // Persist the exact signed quote before the wallet can create an attempt.
      updateRows([...rows]);
      try {
        await sendAgentRuntimeZap({
          quoteEventJson,
          idempotencyKey: row.zapIdempotencyKey,
        });
      } catch (cause) {
        if (walletCommandError(cause).code === "payment_failed") {
          row = {
            ...row,
            zapIdempotencyKey: crypto.randomUUID(),
            quoteEventJson: null,
          };
          rows[index] = row;
          updateRows([...rows]);
        }
        throw cause;
      }
      row = { ...row, paymentSent: true };
      rows[index] = row;
      updateRows([...rows]);
    }
    for (let attempt = 0; result.response.status !== "reserved"; attempt += 1) {
      if (attempt >= 30) {
        throw new Error(
          `${row.name} received payment, but its runtime deposit is still pending. Retry to continue without paying again.`,
        );
      }
      await new Promise((resolve) => window.setTimeout(resolve, 1_000));
      result = await requestAgentRuntimeReservation({
        agentPubkey: row.pubkey,
        channelId: checkout.channelId,
        capMinutes,
        requestId: row.requestId,
      });
      if (result.response.status === "unavailable") {
        throw new Error(`${row.name} is unavailable for paid runtime.`);
      }
    }
    rows[index] = {
      ...row,
      reservationTag: runtimeReservationMessageTag(
        row.pubkey,
        result.response.reservation_event,
      ),
    };
    updateRows([...rows]);
  }
  return rows.flatMap((row) =>
    row.reservationTag ? [row.reservationTag] : [],
  );
}

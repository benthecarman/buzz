import * as React from "react";
import { useRelayAgentsQuery } from "@/features/agents/hooks";
import { useCommunities } from "@/features/communities/useCommunities";
import {
  type AgentRuntimeCapMinutes,
  type AgentRuntimeStatus,
  agentRuntimePackRequired,
  getAgentRuntimeStatus,
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

/**
 * Prepaid checkout: there is no negotiation with the Agent. Each row settles
 * by observing durable state — an open reservation in the payer's own ledger
 * — after at most one payment against the Agent's published terms. Waiting is
 * bounded by the attestation loop (~15s on the owner's desktop) plus the
 * Agent's mint loop (~30s), so the observation window is generous but finite;
 * a timeout leaves the paid credit retained and the checkout resumable.
 */
const RESERVATION_POLL_INTERVAL_MS = 3_000;
const RESERVATION_POLL_BUDGET_MS = 120_000;

type PendingRow = RuntimeCheckoutRow & {
  zapIdempotencyKey: string;
  paymentSent: boolean;
  reservationTag: string[] | null;
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
      const statuses = await Promise.all(
        paidAgents.map((agent) =>
          getAgentRuntimeStatus({ agentPubkey: agent.pubkey, channelId }),
        ),
      );
      let rows = paidAgents
        .map(
          (agent, index): PendingRow => ({
            pubkey: normalizePubkey(agent.pubkey),
            name: agent.name,
            rateSats:
              statuses[index]?.pricing?.rateSatsPerMinute ??
              agent.pricePerMinuteSats,
            availableMs: spendableMs(statuses[index]),
            zapIdempotencyKey: crypto.randomUUID(),
            paymentSent: false,
            reservationTag: null,
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
        // Resume the durable idempotency keys so a renderer restart can never
        // pay the same purchase twice: the wallet replays the same attempt.
        setCapMinutes(stored.capMinutes);
        rows = rows.map((row, index) => ({
          ...row,
          zapIdempotencyKey:
            stored.rows[index]?.zapIdempotencyKey ?? row.zapIdempotencyKey,
          paymentSent: stored.rows[index]?.paymentSent ?? false,
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

/** A claimable lock: open in the ledger and not past its deadline. */
function claimableReservation(
  status: AgentRuntimeStatus,
): AgentRuntimeStatus["openReservation"] {
  const reservation = status.openReservation;
  if (!reservation) return null;
  return reservation.mustStartBy > Math.floor(Date.now() / 1_000)
    ? reservation
    : null;
}

/**
 * What this scope can spend on one more invocation: free credit plus the cap
 * already locked for it. `availableMs` alone would double-charge a payer
 * whose credit sits inside an open reservation. The dialog and the purchase
 * decision must use this same figure or the price shown is not the price
 * charged.
 */
function spendableMs(status: AgentRuntimeStatus | undefined): number {
  if (!status) return 0;
  return status.availableMs + (claimableReservation(status)?.capMs ?? 0);
}

async function completeCheckout(
  checkout: PendingCheckout,
  capMinutes: AgentRuntimeCapMinutes,
  updateRows: (rows: PendingRow[]) => void,
): Promise<string[][]> {
  const rows = [...checkout.rows];
  for (let index = 0; index < rows.length; index += 1) {
    let row = rows[index];
    if (!row || row.reservationTag) continue;

    let status = await getAgentRuntimeStatus({
      agentPubkey: row.pubkey,
      channelId: checkout.channelId,
    });

    // Pay when the scope's spendable credit — free balance plus the open
    // lock's cap, the same figure the dialog displayed — does not cover the
    // selected pack. `paymentSent` survives restarts, and the wallet's
    // idempotency key makes a re-send replay the same attempt, never a
    // second payment.
    const needsPurchase =
      !row.paymentSent &&
      agentRuntimePackRequired(spendableMs(status), capMinutes);
    if (needsPurchase) {
      const pricing = status.pricing;
      if (!pricing) {
        throw new Error(
          `${row.name} does not currently advertise runtime pricing.`,
        );
      }
      try {
        await sendAgentRuntimeZap({
          agentPubkey: row.pubkey,
          channelId: checkout.channelId,
          packMinutes: capMinutes,
          pricingEventJson: pricing.pricingEventJson,
          idempotencyKey: row.zapIdempotencyKey,
        });
      } catch (cause) {
        if (walletCommandError(cause).code === "payment_failed") {
          // A failed payment is terminal for its key; the next attempt needs
          // a fresh one so the wallet does not replay the failure.
          row = { ...row, zapIdempotencyKey: crypto.randomUUID() };
          rows[index] = row;
          updateRows([...rows]);
        }
        throw cause;
      }
      row = { ...row, paymentSent: true };
      rows[index] = row;
      updateRows([...rows]);
    }

    // Observe the ledger until the Agent's lock appears. Attestation and
    // minting run on other machines' timers; the budget covers both plus
    // slack, and expiry leaves credit retained and the checkout resumable.
    const deadline = Date.now() + RESERVATION_POLL_BUDGET_MS;
    let reservation = claimableReservation(status);
    while (!reservation) {
      if (Date.now() > deadline) {
        throw new Error(
          `${row.name} has your payment, but its runtime reservation has not appeared yet. Your credit is retained — retry without paying again.`,
        );
      }
      await new Promise((resolve) =>
        window.setTimeout(resolve, RESERVATION_POLL_INTERVAL_MS),
      );
      status = await getAgentRuntimeStatus({
        agentPubkey: row.pubkey,
        channelId: checkout.channelId,
      });
      reservation = claimableReservation(status);
    }

    rows[index] = {
      ...row,
      availableMs: status.availableMs,
      reservationTag: runtimeReservationMessageTag(
        row.pubkey,
        reservation.reservationEventId,
      ),
    };
    updateRows([...rows]);
  }
  return rows.flatMap((row) =>
    row.reservationTag ? [row.reservationTag] : [],
  );
}

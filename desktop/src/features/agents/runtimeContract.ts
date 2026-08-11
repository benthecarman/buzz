/**
 * The pure half of the paid-runtime payer protocol: everything between the
 * status JSON the Tauri `agent_runtime_get_status` command returns and the
 * `runtimeTags` argument handed back to `send_channel_message`.
 *
 * This module must stay free of Tauri imports — it is exercised directly by
 * `runtimeContract.test.mjs` against `fixtures/agent-runtime-contract.json`,
 * the same fixture the Rust side pins its serialization to. Together the two
 * tests prove the boundary: Rust emits exactly the fixture's status, and this
 * code, given exactly that status, produces the fixture's invocation.
 */

export const AGENT_RUNTIME_CAPS_MINUTES = [15, 30, 60] as const;
export type AgentRuntimeCapMinutes =
  (typeof AGENT_RUNTIME_CAPS_MINUTES)[number];

/**
 * One open, claimable reservation read out of the payer's own ledger.
 *
 * The prepaid protocol has no request/response round: the Agent mints a lock
 * from settled credit on its maintenance loop, and the payer discovers it by
 * reading durable state. Attach `reservationEventId` to the instruction; the
 * relay's claim trigger makes it single-use.
 */
export type AgentRuntimeOpenReservation = {
  reservationEventJson: string;
  reservationEventId: string;
  capMs: number;
  mustStartBy: number;
};

/** The Agent's published terms a new purchase would pay against. */
export type AgentRuntimePricingTerms = {
  /** Exact signed kind-10101 event; pinned verbatim into the zap intent. */
  pricingEventJson: string;
  rateSatsPerMinute: number;
};

export type AgentRuntimeStatus = {
  availableMs: number;
  creditedMs: number;
  usedMs: number;
  openReservation: AgentRuntimeOpenReservation | null;
  pricing: AgentRuntimePricingTerms | null;
};

export function agentRuntimePackRequired(
  availableMs: number,
  capMinutes: AgentRuntimeCapMinutes,
): boolean {
  return availableMs < capMinutes * 60_000;
}

export function agentRuntimePackChargeSats(
  availableMs: number,
  capMinutes: AgentRuntimeCapMinutes,
  rateSatsPerMinute: number,
): number {
  return agentRuntimePackRequired(availableMs, capMinutes)
    ? capMinutes * rateSatsPerMinute
    : 0;
}

/** A claimable lock: open in the ledger and not past its deadline. */
export function claimableReservation(
  status: AgentRuntimeStatus,
): AgentRuntimeOpenReservation | null {
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
export function spendableMs(status: AgentRuntimeStatus | undefined): number {
  if (!status) return 0;
  return status.availableMs + (claimableReservation(status)?.capMs ?? 0);
}

/**
 * The marker the relay claims a runtime reservation against. Rides the
 * dedicated `runtimeTags` send channel — never the imeta media channel,
 * whose guard rejects it and fails the whole message.
 */
export function runtimeReservationMessageTag(
  agentPubkey: string,
  reservationEventId: string,
): string[] {
  if (!/^[0-9a-f]{64}$/u.test(reservationEventId)) {
    throw new Error("Agent minted an invalid runtime reservation event.");
  }
  return ["agent_runtime", agentPubkey, reservationEventId];
}

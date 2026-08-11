import { invokeTauri } from "@/shared/api/tauri";

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

/**
 * Everything the checkout needs about one (agent, channel) scope, read from
 * durable state alone. This is the whole payer protocol: there is nothing to
 * ask the Agent, only state to observe.
 */
export function getAgentRuntimeStatus(input: {
  agentPubkey: string;
  channelId: string;
}): Promise<AgentRuntimeStatus> {
  return invokeTauri<AgentRuntimeStatus>("agent_runtime_get_status", {
    input,
  });
}

export function runtimeReservationMessageTag(
  agentPubkey: string,
  reservationEventId: string,
): string[] {
  if (!/^[0-9a-f]{64}$/u.test(reservationEventId)) {
    throw new Error("Agent minted an invalid runtime reservation event.");
  }
  return ["agent_runtime", agentPubkey, reservationEventId];
}

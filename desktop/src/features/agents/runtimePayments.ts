import { invokeTauri } from "@/shared/api/tauri";

export const AGENT_RUNTIME_CAPS_MINUTES = [15, 30, 60] as const;
export type AgentRuntimeCapMinutes =
  (typeof AGENT_RUNTIME_CAPS_MINUTES)[number];

export type AgentRuntimeQuote = {
  version: 1;
  request_id: string;
  agent_pubkey: string;
  payer_pubkey: string;
  channel_id: string;
  cap_minutes: AgentRuntimeCapMinutes;
  pack_minutes: AgentRuntimeCapMinutes;
  price_per_minute_sats: number;
  amount_sats: number;
  offer_event: Record<string, unknown>;
  expires_at: number;
};

export type AgentRuntimeReservationResponse =
  | {
      version: 1;
      status: "reserved";
      request_id: string;
      reservation_event: Record<string, unknown>;
    }
  | ({ status: "payment_required" } & AgentRuntimeQuote)
  | {
      version: 1;
      status: "unavailable";
      request_id: string;
    };

export type AgentRuntimeReservationResult = {
  requestId: string;
  requestEventId: string;
  responseEventJson: string;
  response: AgentRuntimeReservationResponse;
};

export type AgentRuntimeBalance = {
  availableMs: number;
  creditedMs: number;
  usedMs: number;
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

export function getAgentRuntimeBalance(input: {
  agentPubkey: string;
  channelId: string;
}): Promise<AgentRuntimeBalance> {
  return invokeTauri<AgentRuntimeBalance>("agent_runtime_get_balance", {
    input,
  });
}

export function requestAgentRuntimeReservation(input: {
  agentPubkey: string;
  channelId: string;
  capMinutes: AgentRuntimeCapMinutes;
  requestId?: string;
}): Promise<AgentRuntimeReservationResult> {
  return invokeTauri<AgentRuntimeReservationResult>(
    "agent_runtime_request_reservation",
    { input },
  );
}

export function runtimeReservationMessageTag(
  agentPubkey: string,
  reservationEvent: Record<string, unknown>,
): string[] {
  const reservationId = reservationEvent.id;
  if (typeof reservationId !== "string" || reservationId.length !== 64) {
    throw new Error("Agent returned an invalid runtime reservation event.");
  }
  return ["agent_runtime", agentPubkey, reservationId];
}

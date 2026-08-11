import { invokeTauri } from "@/shared/api/tauri";
import type { AgentRuntimeStatus } from "./runtimeContract";

export {
  AGENT_RUNTIME_CAPS_MINUTES,
  agentRuntimePackChargeSats,
  agentRuntimePackRequired,
  claimableReservation,
  runtimeReservationMessageTag,
  spendableMs,
} from "./runtimeContract";
export type {
  AgentRuntimeCapMinutes,
  AgentRuntimeOpenReservation,
  AgentRuntimePricingTerms,
  AgentRuntimeStatus,
} from "./runtimeContract";

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

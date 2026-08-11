import { invokeTauri } from "@/shared/api/tauri";
import type { AgentRuntimeStatus } from "./runtimeContract";

export {
  activeAccessZap,
  runtimeZapMessageTag,
} from "./runtimeContract";
export type {
  AgentRuntimeAccessZap,
  AgentRuntimePricingTerms,
  AgentRuntimeStatus,
} from "./runtimeContract";

/**
 * Read the current price and a valid access zap for one Agent and channel.
 */
export function getAgentRuntimeStatus(input: {
  agentPubkey: string;
  channelId: string;
}): Promise<AgentRuntimeStatus> {
  return invokeTauri<AgentRuntimeStatus>("agent_runtime_get_status", {
    input,
  });
}

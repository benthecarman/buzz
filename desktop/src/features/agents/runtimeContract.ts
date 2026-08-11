/** The settled zap that grants access to one Agent in one channel. */
export type AgentRuntimeAccessZap = {
  zapEventId: string;
  createdAt: number;
  validUntil: number;
};

/** The Agent's current flat-price invocation terms. */
export type AgentRuntimePricingTerms = {
  pricingEventJson: string;
  priceSats: number;
  invocationWindowSeconds: number;
};

export type AgentRuntimeStatus = {
  accessZap: AgentRuntimeAccessZap | null;
  pricing: AgentRuntimePricingTerms | null;
};

export function activeAccessZap(
  status: AgentRuntimeStatus | undefined,
  nowSeconds = Math.floor(Date.now() / 1_000),
): AgentRuntimeAccessZap | null {
  const zap = status?.accessZap;
  return zap && nowSeconds <= zap.validUntil ? zap : null;
}

export function runtimeZapMessageTag(
  agentPubkey: string,
  zapEventId: string,
): string[] {
  if (!/^[0-9a-f]{64}$/u.test(zapEventId)) {
    throw new Error("The Agent access zap has an invalid event ID.");
  }
  return ["agent_runtime", agentPubkey, zapEventId];
}

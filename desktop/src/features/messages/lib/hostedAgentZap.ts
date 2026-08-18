import type { TimelineMessage } from "@/features/messages/types";

export type HostedAgentZapTarget = {
  amount: number;
  channelId: string;
  leaseId: string | null;
  targetEventId: string;
};

export type HostedAgentPlanMessage = {
  channelId: string;
  harnessProfile: string;
  hourlyPriceSats: number;
  name: string;
  retentionDays: number;
  targetEventId: string;
};

const MAX_HOURLY_PRICE_SATS = 9_007_199_254_740;

function integerInRange(
  value: unknown,
  minimum: number,
  maximum: number,
): value is number {
  return (
    Number.isSafeInteger(value) &&
    Number(value) >= minimum &&
    Number(value) <= maximum
  );
}

function boundedText(value: unknown, maximumBytes: number): value is string {
  return (
    typeof value === "string" &&
    value.trim().length > 0 &&
    new TextEncoder().encode(value).length <= maximumBytes
  );
}

/** Read and validate a hosted-agent plan from a normal channel message. */
export function hostedAgentPlanMessage(
  message: TimelineMessage,
  currentChannelId?: string | null,
): HostedAgentPlanMessage | null {
  const planTags = message.tags?.filter((tag) => tag[0] === "agent_host_plan");
  const channelTags = message.tags?.filter((tag) => tag[0] === "h");
  if (
    planTags?.length === 1 &&
    channelTags?.length === 1 &&
    currentChannelId &&
    channelTags[0][1] === currentChannelId
  ) {
    try {
      const plan = JSON.parse(planTags[0][1] ?? "") as {
        harness_profile?: unknown;
        version?: unknown;
        name?: unknown;
        hourly_price_sats?: unknown;
        retention_days?: unknown;
      };
      if (
        plan.version === 1 &&
        boundedText(plan.name, 80) &&
        integerInRange(plan.hourly_price_sats, 1, MAX_HOURLY_PRICE_SATS) &&
        integerInRange(plan.retention_days, 1, 365) &&
        boundedText(plan.harness_profile, 80)
      ) {
        return {
          channelId: currentChannelId,
          harnessProfile: plan.harness_profile,
          hourlyPriceSats: plan.hourly_price_sats,
          name: plan.name,
          retentionDays: plan.retention_days,
          targetEventId: message.id,
        };
      }
    } catch {
      return null;
    }
  }

  return null;
}

/** Read the fixed purchase details from a hosted-agent plan or receipt. */
export function hostedAgentZapTarget(
  message: TimelineMessage,
  currentChannelId?: string | null,
): HostedAgentZapTarget | null {
  const plan = hostedAgentPlanMessage(message, currentChannelId);
  if (plan) {
    return {
      amount: plan.hourlyPriceSats,
      channelId: plan.channelId,
      leaseId: null,
      targetEventId: plan.targetEventId,
    };
  }

  const receiptTags = message.tags?.filter(
    (tag) => tag[0] === "hosted_agent_receipt",
  );
  if (receiptTags?.length !== 1) return null;
  try {
    const receipt = JSON.parse(receiptTags[0][1] ?? "") as {
      channel_id?: unknown;
      hourly_price_sats?: unknown;
      lease_id?: unknown;
      plan_event_id?: unknown;
    };
    if (
      typeof receipt.channel_id === "string" &&
      integerInRange(receipt.hourly_price_sats, 1, MAX_HOURLY_PRICE_SATS) &&
      typeof receipt.lease_id === "string" &&
      typeof receipt.plan_event_id === "string"
    ) {
      return {
        amount: receipt.hourly_price_sats,
        channelId: receipt.channel_id,
        leaseId: receipt.lease_id,
        targetEventId: receipt.plan_event_id,
      };
    }
  } catch {
    return null;
  }
  return null;
}

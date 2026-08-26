import type { TimelineMessage } from "@/features/messages/types";
import {
  KIND_HOSTED_AGENT_PLAN,
  KIND_STREAM_MESSAGE,
  KIND_STREAM_MESSAGE_V2,
} from "@/shared/constants/kinds";

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
  model: string;
  name: string;
  retentionDays: number;
  systemPrompt: string;
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

/** Read and validate a parameterized-replaceable hosted-agent plan. */
export function hostedAgentPlanMessage(
  message: TimelineMessage,
  currentChannelId?: string | null,
): HostedAgentPlanMessage | null {
  const planTags = message.tags?.filter((tag) => tag[0] === "agent_host_plan");
  const channelTags = message.tags?.filter((tag) => tag[0] === "h");
  const identifierTags = message.tags?.filter((tag) => tag[0] === "d");
  const referenceTags = message.tags?.filter(
    (tag) => tag[0] === "agent_host_plan_ref",
  );
  const isPlanAnnouncement =
    message.kind === KIND_HOSTED_AGENT_PLAN &&
    identifierTags?.length === 1 &&
    identifierTags[0][1] === "hosted-agent";
  const isPlanReply =
    (message.kind === KIND_STREAM_MESSAGE ||
      message.kind === KIND_STREAM_MESSAGE_V2) &&
    referenceTags?.length === 1 &&
    referenceTags[0].length === 3 &&
    Boolean(referenceTags[0][1]) &&
    Boolean(referenceTags[0][2]);
  if (
    planTags?.length === 1 &&
    channelTags?.length === 1 &&
    currentChannelId &&
    channelTags[0][1] === currentChannelId &&
    (isPlanAnnouncement || isPlanReply)
  ) {
    try {
      const plan = JSON.parse(planTags[0][1] ?? "") as {
        harness_profile?: unknown;
        version?: unknown;
        name?: unknown;
        hourly_price_sats?: unknown;
        model?: unknown;
        retention_days?: unknown;
        system_prompt?: unknown;
      };
      const model = boundedText(plan.model, 128) ? plan.model : null;
      const systemPrompt = boundedText(plan.system_prompt, 16 * 1024)
        ? plan.system_prompt
        : null;
      const hasCurrentAgentTerms =
        plan.version === 1 && model !== null && systemPrompt !== null;
      if (
        hasCurrentAgentTerms &&
        boundedText(plan.name, 80) &&
        integerInRange(plan.hourly_price_sats, 1, MAX_HOURLY_PRICE_SATS) &&
        integerInRange(plan.retention_days, 1, 365) &&
        boundedText(plan.harness_profile, 80)
      ) {
        return {
          channelId: isPlanReply
            ? (referenceTags[0][2] as string)
            : currentChannelId,
          harnessProfile: plan.harness_profile,
          hourlyPriceSats: plan.hourly_price_sats,
          model,
          name: plan.name,
          retentionDays: plan.retention_days,
          systemPrompt,
          targetEventId: isPlanReply
            ? (referenceTags[0][1] as string)
            : message.id,
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

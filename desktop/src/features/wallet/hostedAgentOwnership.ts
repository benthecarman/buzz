import type { Channel, RelayEvent } from "@/shared/api/types";
import type { RelaySubscriptionFilter } from "@/shared/api/relayClientShared";
import {
  KIND_BOLT12_ZAP,
  KIND_STREAM_MESSAGE_V2,
} from "@/shared/constants/kinds";

const OWNERSHIP_CATCHUP_LIMIT = 1_000;

function values(event: RelayEvent, name: string): string[][] {
  return event.tags.filter((tag) => tag[0] === name);
}

export function hostedAgentDmChannelKey(
  channels: Pick<Channel, "channelType" | "id" | "isMember">[],
): string {
  return channels
    .filter((channel) => channel.channelType === "dm" && channel.isMember)
    .map((channel) => channel.id)
    .sort()
    .join(",");
}

export function ownershipClaimRequestReference(
  request: RelayEvent,
  marker: "plan" | "zap",
): string | null {
  const matches = values(request, "e").filter((tag) => tag[3] === marker);
  return matches.length === 1 ? (matches[0][1] ?? null) : null;
}

export function ownershipClaimLeaseId(request: RelayEvent): string | null {
  const matches = values(request, "d");
  return matches.length === 1 ? (matches[0][1] ?? null) : null;
}

/** Identify factory claim requests before doing purchase-proof lookups. */
export function isHostedAgentOwnershipRequest(
  request: RelayEvent,
  buyerPubkey: string,
): boolean {
  return (
    request.kind === KIND_STREAM_MESSAGE_V2 &&
    Boolean(ownershipClaimLeaseId(request)) &&
    values(request, "h").length === 1 &&
    Boolean(values(request, "h")[0][1]) &&
    values(request, "agent").length === 1 &&
    Boolean(values(request, "agent")[0][1]) &&
    values(request, "name").length === 1 &&
    Boolean(values(request, "name")[0][1]?.trim()) &&
    values(request, "p").length === 1 &&
    values(request, "p")[0][1] === buyerPubkey &&
    ownershipClaimRequestReference(request, "plan") !== null &&
    ownershipClaimRequestReference(request, "zap") !== null
  );
}

export function hostedAgentOwnershipCatchupFilter(
  buyerPubkey: string,
): RelaySubscriptionFilter {
  return {
    kinds: [KIND_STREAM_MESSAGE_V2],
    "#p": [buyerPubkey],
    limit: OWNERSHIP_CATCHUP_LIMIT,
  };
}

/** Replay stored addressed messages without waiting for every DM subscription. */
export async function catchUpHostedAgentOwnershipRequests(input: {
  buyerPubkey: string;
  fetchEvents: (filter: RelaySubscriptionFilter) => Promise<RelayEvent[]>;
  onRequest: (request: RelayEvent) => Promise<void>;
}): Promise<void> {
  const events = await input.fetchEvents(
    hostedAgentOwnershipCatchupFilter(input.buyerPubkey),
  );
  for (const event of events) {
    if (isHostedAgentOwnershipRequest(event, input.buyerPubkey)) {
      await input.onRequest(event);
    }
  }
}

/** Match a factory claim request to the buyer's stored zap proof. */
export function matchesHostedAgentPurchase(
  request: RelayEvent,
  zap: RelayEvent,
  buyerPubkey: string,
): boolean {
  const planId = ownershipClaimRequestReference(request, "plan");
  const zapId = ownershipClaimRequestReference(request, "zap");
  return (
    isHostedAgentOwnershipRequest(request, buyerPubkey) &&
    zap.kind === KIND_BOLT12_ZAP &&
    zap.id === zapId &&
    zap.pubkey === buyerPubkey &&
    values(zap, "p").some((tag) => tag[1] === request.pubkey) &&
    planId !== null &&
    values(zap, "e").some((tag) => tag[1] === planId)
  );
}

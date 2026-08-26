import type { RelayEvent } from "@/shared/api/types";
import { KIND_BOLT12_ZAP } from "@/shared/constants/kinds";
import type { WalletVerifiedZapEvent } from "../types";

function tagValue(event: RelayEvent, name: string): string | null {
  return event.tags.find((tag) => tag[0] === name)?.[1] ?? null;
}

/** Extract display fields from a zap that the relay already validated. */
export function parseRelayZapEvent(
  event: RelayEvent,
): WalletVerifiedZapEvent | null {
  if (event.kind !== KIND_BOLT12_ZAP) return null;

  const amountText = tagValue(event, "amount");
  const description = tagValue(event, "description");
  const recipientPubkey = tagValue(event, "p")?.toLowerCase();
  if (!amountText || !description || !recipientPubkey) return null;

  const amountMsats = Number(amountText);
  if (
    !Number.isSafeInteger(amountMsats) ||
    amountMsats <= 0 ||
    amountMsats % 1_000 !== 0
  ) {
    return null;
  }

  try {
    const intent = JSON.parse(description) as {
      id?: unknown;
      content?: unknown;
    };
    if (typeof intent.id !== "string" || typeof intent.content !== "string") {
      return null;
    }
    return {
      eventId: event.id,
      amount: amountMsats / 1_000,
      comment: intent.content,
      intentEventId: intent.id,
      recipientPubkey,
      targetEventId: tagValue(event, "e"),
      channelId: tagValue(event, "h"),
    };
  } catch {
    return null;
  }
}

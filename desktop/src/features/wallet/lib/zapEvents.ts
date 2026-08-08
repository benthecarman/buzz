import { verifyEvent } from "nostr-tools/pure";

import type { RelayEvent } from "@/shared/api/types";
import {
  KIND_BOLT12_OFFER,
  KIND_BOLT12_ZAP,
  KIND_BOLT12_ZAP_INTENT,
} from "@/shared/constants/kinds";

export type VerifiedZapEvent = {
  amount: number;
  comment: string;
  intentEventId: string;
  recipientPubkey: string;
  targetEventId: string | null;
};

export const PLACEHOLDER_PAYER_PROOF = "placeholder";

function exactTag(tags: string[][], name: string): string | null {
  const matches = tags.filter((tag) => tag[0] === name);
  return matches.length === 1 ? (matches[0]?.[1] ?? null) : null;
}

function optionalTag(
  tags: string[][],
  name: string,
): string | null | undefined {
  const matches = tags.filter((tag) => tag[0] === name);
  if (matches.length > 1) return undefined;
  return matches[0]?.[1] ?? null;
}

const BECH32_ALPHABET = "qpzry9x8gf2tvdw0s3jn54khce6mua7l";

function validBech32(value: string, expectedPrefix: string): boolean {
  if (!value || value !== value.toLowerCase() || value.length > 100_000) {
    return false;
  }
  const separator = value.lastIndexOf("1");
  if (separator < 1 || value.slice(0, separator) !== expectedPrefix)
    return false;
  const data = value.slice(separator + 1);
  if (data.length < 6) return false;
  const values = [
    ...Array.from(expectedPrefix, (character) => character.charCodeAt(0) >> 5),
    0,
    ...Array.from(expectedPrefix, (character) => character.charCodeAt(0) & 31),
    ...Array.from(data, (character) => BECH32_ALPHABET.indexOf(character)),
  ];
  if (values.some((word) => word < 0)) return false;
  const generators = [
    0x3b6a57b2, 0x26508e6d, 0x1ea119fa, 0x3d4233dd, 0x2a1462b3,
  ];
  let checksum = 1;
  for (const value of values) {
    const top = checksum >>> 25;
    checksum = ((checksum & 0x1ffffff) << 5) ^ value;
    for (let index = 0; index < generators.length; index += 1) {
      if ((top >>> index) & 1) checksum ^= generators[index] ?? 0;
    }
  }
  return checksum >>> 0 === 1;
}

function sameOptionalTag(
  outer: string[][],
  intent: string[][],
  name: string,
): boolean {
  const outerValue = optionalTag(outer, name);
  const intentValue = optionalTag(intent, name);
  return (
    outerValue !== undefined &&
    intentValue !== undefined &&
    outerValue === intentValue
  );
}

function parseSignedEvent(value: string): RelayEvent | null {
  try {
    const event = JSON.parse(value) as RelayEvent;
    return verifyEvent(event) ? event : null;
  } catch {
    return null;
  }
}

function canonicalOfferAnnouncement(event: RelayEvent, recipient: string) {
  if (
    event.kind !== KIND_BOLT12_OFFER ||
    event.pubkey.toLowerCase() !== recipient ||
    !verifyEvent(event)
  ) {
    return false;
  }
  const offers = event.tags
    .filter((tag) => tag[0] === "offer")
    .map((tag) => tag[1]);
  return (
    offers.length > 0 &&
    offers.every(
      (offer) =>
        typeof offer === "string" &&
        validBech32(offer, "lno") &&
        offer === offer.toLowerCase() &&
        !/\s/.test(offer),
    )
  );
}

/**
 * Validate the NIP-B1 Nostr proof chain and event structure before correlating
 * it with wallet settlement. Cryptographic payer-proof validation is deferred.
 */
export function parseTaggedZapEvent(
  event: RelayEvent,
  allowedRecipients?: ReadonlySet<string>,
): VerifiedZapEvent | null {
  if (event.kind !== KIND_BOLT12_ZAP || !verifyEvent(event)) return null;

  const recipient = exactTag(event.tags, "p")?.toLowerCase() ?? null;
  const amountText = exactTag(event.tags, "amount");
  const description = exactTag(event.tags, "description");
  const offerEventJson = exactTag(event.tags, "offer_event");
  const proof = exactTag(event.tags, "proof");
  if (
    !recipient ||
    (allowedRecipients !== undefined && !allowedRecipients.has(recipient)) ||
    !amountText ||
    !description ||
    !offerEventJson ||
    !proof ||
    (proof !== PLACEHOLDER_PAYER_PROOF && !validBech32(proof, "lnp"))
  ) {
    return null;
  }

  const amountMsats = Number(amountText);
  if (
    !Number.isSafeInteger(amountMsats) ||
    amountMsats <= 0 ||
    amountMsats % 1_000 !== 0
  ) {
    return null;
  }
  const intent = parseSignedEvent(description);
  const offerEvent = parseSignedEvent(offerEventJson);
  const zapId = intent ? exactTag(intent.tags, "zap_id") : null;
  const outerEventTarget = optionalTag(event.tags, "e");
  const outerAddressTarget = optionalTag(event.tags, "a");
  if (
    !intent ||
    !offerEvent ||
    intent.kind !== KIND_BOLT12_ZAP_INTENT ||
    intent.pubkey !== event.pubkey ||
    intent.content !== event.content ||
    exactTag(intent.tags, "p")?.toLowerCase() !== recipient ||
    exactTag(intent.tags, "amount") !== amountText ||
    exactTag(intent.tags, "offer_event") !== offerEventJson ||
    !zapId ||
    !/^[0-9a-f]{32,}$/.test(zapId) ||
    !sameOptionalTag(event.tags, intent.tags, "e") ||
    !sameOptionalTag(event.tags, intent.tags, "a") ||
    !sameOptionalTag(event.tags, intent.tags, "k") ||
    outerEventTarget === undefined ||
    outerAddressTarget === undefined ||
    (outerEventTarget !== null && outerAddressTarget !== null) ||
    !canonicalOfferAnnouncement(offerEvent, recipient) ||
    offerEvent.created_at > intent.created_at
  ) {
    return null;
  }
  const payer = optionalTag(event.tags, "P");
  if (
    payer === undefined ||
    (payer !== null && payer.toLowerCase() !== event.pubkey.toLowerCase())
  ) {
    return null;
  }

  return {
    amount: amountMsats / 1_000,
    comment: intent.content,
    intentEventId: intent.id,
    recipientPubkey: recipient,
    targetEventId: outerEventTarget,
  };
}

export function zapSubscriptionFilter(
  pubkeys: readonly string[],
  since: number,
) {
  return {
    kinds: [KIND_BOLT12_ZAP],
    "#p": [
      ...new Set(
        pubkeys.map((pubkey) => pubkey.trim().toLowerCase()).filter(Boolean),
      ),
    ],
    limit: 50,
    since,
  };
}

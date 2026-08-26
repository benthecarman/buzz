import type { RelayEvent } from "@/shared/api/types";
import { KIND_BOLT12_ZAP } from "@/shared/constants/kinds";
import type { WalletVerifiedZapEvent } from "../types";

const INVOICE_PAYMENT_HASH_TYPE = 168n;
const BECH32_CHARSET = "qpzry9x8gf2tvdw0s3jn54khce6mua7l";

function tagValue(event: RelayEvent, name: string): string | null {
  return event.tags.find((tag) => tag[0] === name)?.[1] ?? null;
}

function optionalEventKind(event: RelayEvent): number | null {
  const value = tagValue(event, "k");
  if (value === null) return null;
  const kind = Number(value);
  return Number.isSafeInteger(kind) && kind >= 0 ? kind : null;
}

function readBigSize(bytes: Uint8Array, offset: number) {
  const first = bytes[offset];
  if (first === undefined) return null;
  if (first < 0xfd) return { nextOffset: offset + 1, value: BigInt(first) };
  const byteCount = first === 0xfd ? 2 : first === 0xfe ? 4 : 8;
  if (offset + 1 + byteCount > bytes.length) return null;
  let value = 0n;
  for (let index = 0; index < byteCount; index += 1) {
    value = (value << 8n) | BigInt(bytes[offset + 1 + index]);
  }
  const minimum =
    first === 0xfd ? 0xfdn : first === 0xfe ? 0x10000n : 0x100000000n;
  if (value < minimum) return null;
  return { nextOffset: offset + 1 + byteCount, value };
}

function bytesToHex(bytes: Uint8Array) {
  return [...bytes].map((byte) => byte.toString(16).padStart(2, "0")).join("");
}

function decodeBolt12Data(value: string): Uint8Array | null {
  if (!value.startsWith("lnp1") || value !== value.toLowerCase()) return null;
  const words = [...value.slice(4)].map((character) =>
    BECH32_CHARSET.indexOf(character),
  );
  if (words.length === 0 || words.some((word) => word < 0)) return null;

  const bytes: number[] = [];
  let accumulator = 0;
  let bitCount = 0;
  for (const word of words) {
    accumulator = (accumulator << 5) | word;
    bitCount += 5;
    while (bitCount >= 8) {
      bitCount -= 8;
      bytes.push((accumulator >> bitCount) & 0xff);
    }
    accumulator &= (1 << bitCount) - 1;
  }
  if (bitCount >= 5 || accumulator !== 0) return null;
  return Uint8Array.from(bytes);
}

/** Read the payment hash from a relay-validated BOLT12 payer proof. */
export function payerProofPaymentHash(proof: string): string | null {
  const bytes = decodeBolt12Data(proof);
  if (!bytes) return null;
  try {
    let offset = 0;
    while (offset < bytes.length) {
      const type = readBigSize(bytes, offset);
      if (!type) return null;
      const length = readBigSize(bytes, type.nextOffset);
      if (!length || length.value > BigInt(Number.MAX_SAFE_INTEGER))
        return null;
      const valueOffset = length.nextOffset;
      const end = valueOffset + Number(length.value);
      if (end > bytes.length) return null;
      if (type.value === INVOICE_PAYMENT_HASH_TYPE) {
        return length.value === 32n
          ? bytesToHex(bytes.subarray(valueOffset, end))
          : null;
      }
      offset = end;
    }
  } catch {
    return null;
  }
  return null;
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
      paymentHash: payerProofPaymentHash(tagValue(event, "proof") ?? ""),
      targetEventId: tagValue(event, "e"),
      targetEventKind: optionalEventKind(event),
      channelId: tagValue(event, "h"),
      leaseId: tagValue(event, "lease"),
    };
  } catch {
    return null;
  }
}

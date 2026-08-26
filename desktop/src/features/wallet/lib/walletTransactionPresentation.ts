import type { RelayEvent } from "@/shared/api/types";
import { KIND_HOSTED_AGENT_PLAN } from "@/shared/constants/kinds";
import { truncatePubkey } from "@/shared/lib/pubkey";
import type { WalletTransaction } from "../types";
import type { ZapHistoryItem } from "./zapHistory";

export type WalletTransactionContext = {
  channelNames: ReadonlyMap<string, string>;
  ownedAgentPubkeys: ReadonlySet<string>;
  ownerPubkey: string | null;
  targetEvents: ReadonlyMap<string, RelayEvent>;
  userNames: ReadonlyMap<string, string>;
  zapsByIntent: ReadonlyMap<string, ZapHistoryItem>;
  zapsByPaymentHash: ReadonlyMap<string, ZapHistoryItem>;
};

export type WalletTransactionPresentation = {
  description: string;
  title: string;
};

const PAYER_NOTE_PREFIX = "nostr:nipB1:";
const INTERNAL_NOTE =
  /^Buzz (?:hosted agent payment|message zap|profile payment)\s+\S+$/;

function intentEventId(transaction: WalletTransaction): string | null {
  const payerNote = transaction.payerNote?.trim();
  if (payerNote?.startsWith(PAYER_NOTE_PREFIX)) {
    return payerNote.slice(PAYER_NOTE_PREFIX.length) || null;
  }
  const note = transaction.note?.trim();
  return note?.match(INTERNAL_NOTE)?.[0].split(/\s+/).at(-1) ?? null;
}

function mention(
  pubkey: string,
  names: ReadonlyMap<string, string>,
  fallbackName?: string,
): string {
  const name = names.get(pubkey.trim().toLowerCase())?.trim();
  const label = name || fallbackName?.trim() || truncatePubkey(pubkey);
  return `@${label.replace(/^@/, "")}`;
}

function channelLabel(
  channelId: string | null,
  names: ReadonlyMap<string, string>,
): string {
  if (!channelId) return "a channel";
  const name = names.get(channelId)?.trim();
  return name ? `#${name.replace(/^#/, "")}` : "a channel";
}

export function hostedAgentPlanName(event: RelayEvent | undefined) {
  if (
    event?.kind !== KIND_HOSTED_AGENT_PLAN ||
    !event.tags.some((tag) => tag[0] === "d" && tag[1] === "hosted-agent")
  ) {
    return null;
  }
  const plan = event?.tags.find((tag) => tag[0] === "agent_host_plan")?.[1];
  if (!plan) return null;
  try {
    const value = JSON.parse(plan) as { name?: unknown };
    return typeof value.name === "string" && value.name.trim()
      ? value.name.trim()
      : null;
  } catch {
    return null;
  }
}

function fallbackPresentation(
  transaction: WalletTransaction,
): WalletTransactionPresentation {
  const direction =
    transaction.direction === "inbound"
      ? "received"
      : transaction.direction === "outbound"
        ? "sent"
        : transaction.direction;
  const note = transaction.note?.trim();
  return {
    title: `Payment ${direction}`,
    description:
      note && !INTERNAL_NOTE.test(note) && !note.startsWith(PAYER_NOTE_PREFIX)
        ? note
        : transaction.statusMessage,
  };
}

/** Build wallet copy from a transaction and its cached public zap proof. */
export function walletTransactionPresentation(
  transaction: WalletTransaction,
  context: WalletTransactionContext,
): WalletTransactionPresentation {
  const intentId = intentEventId(transaction);
  const paymentHash = transaction.paymentHash?.trim().toLowerCase();
  const zap =
    (intentId ? context.zapsByIntent.get(intentId) : undefined) ??
    (paymentHash ? context.zapsByPaymentHash.get(paymentHash) : undefined);
  if (
    !zap ||
    (transaction.direction !== "inbound" &&
      transaction.direction !== "outbound")
  ) {
    return fallbackPresentation(transaction);
  }

  const sent = transaction.direction === "outbound";
  const counterparty = mention(
    sent ? zap.recipientPubkey : zap.payerPubkey,
    context.userNames,
  );
  const title = sent ? "Zap sent" : "Zap received";
  const cachedRecipientName = zap.recipientName.trim();
  const receivingAgent =
    !sent &&
    zap.recipientPubkey !== context.ownerPubkey &&
    (cachedRecipientName || context.ownedAgentPubkeys.has(zap.recipientPubkey))
      ? mention(zap.recipientPubkey, context.userNames, cachedRecipientName)
      : null;
  const receivedDescription = receivingAgent
    ? `Zap received by ${receivingAgent} from ${counterparty}`
    : `Zap received from ${counterparty}`;
  const agentName = zap.targetEventId
    ? hostedAgentPlanName(context.targetEvents.get(zap.targetEventId))
    : null;
  const isHostedAgentLease = zap.leaseId !== null || agentName !== null;

  if (isHostedAgentLease) {
    return {
      title,
      description: agentName
        ? `${sent ? `Zap sent to ${counterparty}` : receivedDescription} to lease agent @${agentName.replace(/^@/, "")}`
        : `${sent ? `Zap sent to ${counterparty}` : receivedDescription} to lease an agent`,
    };
  }
  if (zap.targetEventId) {
    const channel = channelLabel(zap.channelId, context.channelNames);
    return {
      title,
      description: sent
        ? `Zap sent to ${counterparty}'s message in ${channel}`
        : `${receivedDescription} on message in ${channel}`,
    };
  }
  return {
    title,
    description: sent ? `Zap sent to ${counterparty}` : receivedDescription,
  };
}

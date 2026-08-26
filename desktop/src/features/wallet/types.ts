import type { RelayEvent } from "@/shared/api/types";

export interface WalletStatus {
  providerName: string;
  balance: number;
  spendableBalance: number;
  lightningBalance: number;
  onchainBalance: number;
}

export interface WalletEnableResult {
  status: WalletStatus;
  publicationWarnings: string[];
}

export interface WalletOfferPublicationResult {
  offer: string | null;
  publicationWarnings: string[];
}

export interface WalletFundingRequest {
  bip321Uri: string;
  bolt11Invoice: string;
  bolt11ExpiresAtMs: number;
  bolt12Offer: string;
}

export interface WalletDestinationAnalysis {
  normalizedDestination: string;
  instructionType: string;
  description: string | null;
  amount: number | null;
  minAmount: number | null;
  maxAmount: number | null;
  expiresAtMs: number | null;
}

export interface WalletSendRequest {
  destination: string;
  amount: number | null;
  message: string | null;
  requestId: string;
}

export interface WalletPaymentResult {
  paymentId: string;
  status: "pending" | "completed" | "failed";
  statusMessage: string;
  preimage?: string | null;
  payerProof?: string | null;
  txid?: string | null;
  amount: number | null;
  fees: number;
  createdAtMs: number;
  finalizedAtMs: number | null;
}

export interface WalletTransaction {
  id: string;
  direction: string;
  status: string;
  statusMessage: string;
  amount: number | null;
  fees: number;
  note: string | null;
  payerNote: string | null;
  offerId: string | null;
  paymentHash: string | null;
  createdAtMs: number;
  finalizedAtMs: number | null;
}

export interface WalletTransactionPage {
  transactions: WalletTransaction[];
  nextCursor: string | null;
}

export interface WalletIncomingPaymentEvent {
  transaction: WalletTransaction;
  status: WalletStatus;
  transactions: WalletTransaction[];
}

export interface WalletRecipientOffer {
  recipientPubkey: string;
  offer: string;
  offerEventJson: string;
  offerEventId: string;
}

/** Display fields from a NIP-B1 zap that the relay validated. */
export interface WalletVerifiedZapEvent {
  eventId: string;
  amount: number;
  comment: string;
  intentEventId: string;
  recipientPubkey: string;
  paymentHash: string | null;
  targetEventId: string | null;
  targetEventKind: number | null;
  channelId: string | null;
  leaseId: string | null;
}

export interface WalletProfileZapRequest {
  recipientPubkey: string;
  amount: number;
  comment: string | null;
  idempotencyKey: string;
  targetEventId?: string | null;
  targetEventKind?: number | null;
  /** Source channel for a hosted-agent plan zap. */
  channelId?: string | null;
  /** Existing lease ID for a hosted-agent renewal. */
  leaseId?: string | null;
}

export type WalletProfileZapDraft = WalletProfileZapRequest;

export interface WalletProfileZapResult {
  payment: WalletPaymentResult;
  intentEventId: string;
  proofEventId: string | null;
  proofCreatedAtSeconds: number | null;
  proofPublished: boolean;
}

export interface WalletNwcRequest {
  eventId: string;
  expiresAtMs: number;
  agentPubkey: string;
  agentName: string;
  requestType: "payment" | "zap";
  instructionType: string;
  recipientPubkey: string | null;
  amount: number;
  comment: string;
  destination: string;
  payerNote: string | null;
  requestId: string;
}

export type WalletNwcBudgetPeriod = "hour" | "day" | "week" | "month";

export interface WalletNwcClient {
  agentPubkey: string;
  agentName: string;
  mode: "manual" | "budget";
  budgetAmount: number | null;
  budgetPeriod: WalletNwcBudgetPeriod | null;
  spentAmount: number;
  remainingAmount: number | null;
  periodEndsAtMs: number | null;
}

export interface WalletNwcPolicyUpdate {
  agentPubkey: string;
  mode: "manual" | "budget";
  budgetAmount: number | null;
  budgetPeriod: WalletNwcBudgetPeriod | null;
}

/** Default policy applied to agents created or claimed later. */
export interface WalletNwcDefaultPolicy {
  mode: "manual" | "budget";
  budgetAmount: number | null;
  budgetPeriod: WalletNwcBudgetPeriod | null;
}

export interface WalletNwcHandlingResult {
  action:
    | "approval_required"
    | "respond"
    | "payment_completed"
    | "payment_pending"
    | "payment_failed";
  request: WalletNwcRequest | null;
  response: RelayEvent | null;
}

export interface WalletCommandError {
  code?: string;
  message?: string;
}

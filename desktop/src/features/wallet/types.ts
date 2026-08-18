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
  status: string;
  statusMessage: string;
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

/** A NIP-B1 zap validated natively, including rust-lightning offer parsing. */
export interface WalletVerifiedZapEvent {
  eventId: string;
  amount: number;
  comment: string;
  intentEventId: string;
  recipientPubkey: string;
  targetEventId: string | null;
  channelId: string | null;
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
  agentPubkey: string;
  agentName: string;
  recipientPubkey: string;
  amount: number;
  comment: string;
  destination: string;
  payerNote: string;
  requestId: string;
}

export interface WalletCommandError {
  code?: string;
  message?: string;
}

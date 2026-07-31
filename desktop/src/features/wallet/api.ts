import { invoke } from "@tauri-apps/api/core";

import type {
  WalletDestinationAnalysis,
  WalletEnableResult,
  WalletFundingRequest,
  WalletOfferPublicationResult,
  WalletPaymentResult,
  WalletProfileZapDraft,
  WalletProfileZapRequest,
  WalletProfileZapResult,
  WalletRecipientOffer,
  WalletSendRequest,
  WalletStatus,
  WalletTransactionPage,
} from "./types";

let compileEnabledPromise: Promise<boolean> | null = null;

export function bitcoinCompileEnabled(): Promise<boolean> {
  compileEnabledPromise ??= invoke<boolean>("bitcoin_compile_enabled");
  return compileEnabledPromise;
}

export function enableWallet(): Promise<WalletEnableResult> {
  return invoke<WalletEnableResult>("wallet_enable");
}

export function disableWallet(): Promise<WalletOfferPublicationResult> {
  return invoke<WalletOfferPublicationResult>("wallet_disable");
}

export function getWalletStatus(): Promise<WalletStatus> {
  return invoke<WalletStatus>("wallet_get_status");
}

export function createWalletReceiveRequest(): Promise<WalletFundingRequest> {
  return invoke<WalletFundingRequest>("wallet_create_receive_request");
}

export function refreshWalletOffer(): Promise<WalletOfferPublicationResult> {
  return invoke<WalletOfferPublicationResult>("wallet_refresh_offer");
}

export function analyzeWalletDestination(
  destination: string,
): Promise<WalletDestinationAnalysis> {
  return invoke<WalletDestinationAnalysis>("wallet_analyze_destination", {
    destination,
  });
}

export function getPendingWalletSend(): Promise<WalletSendRequest | null> {
  return invoke<WalletSendRequest | null>("wallet_get_pending_send");
}

export function sendWalletPayment(
  request: WalletSendRequest,
): Promise<WalletPaymentResult> {
  return invoke<WalletPaymentResult>("wallet_send", { request });
}

export function listWalletTransactions(
  cursor?: string,
  sync = true,
): Promise<WalletTransactionPage> {
  return invoke<WalletTransactionPage>("wallet_list_transactions", {
    cursor: cursor ?? null,
    limit: 25,
    sync,
  });
}

export function pollWalletUpdates(): Promise<boolean> {
  return invoke<boolean>("wallet_poll_updates");
}

export function revealWalletRecoveryPhrase(): Promise<string> {
  return invoke<string>("wallet_reveal_recovery_phrase");
}

export function getRecipientWalletOffer(
  recipientPubkey: string,
): Promise<WalletRecipientOffer> {
  return invoke<WalletRecipientOffer>("wallet_get_recipient_offer", {
    recipientPubkey,
  });
}

export function getPendingProfileZap(
  recipientPubkey: string,
): Promise<WalletProfileZapDraft | null> {
  return invoke<WalletProfileZapDraft | null>(
    "wallet_get_pending_profile_zap",
    {
      recipientPubkey,
    },
  );
}

export function sendProfileZap(
  request: WalletProfileZapRequest,
): Promise<WalletProfileZapResult> {
  return invoke<WalletProfileZapResult>("wallet_send_profile_zap", {
    request,
  });
}

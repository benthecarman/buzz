import { invoke } from "@tauri-apps/api/core";

import { loadCommunities } from "@/features/communities/communityStorage";
import type {
  WalletDestinationAnalysis,
  WalletAgentRuntimeZapRequest,
  WalletEnableResult,
  WalletFundingRequest,
  WalletOfferPublicationResult,
  WalletPaymentResult,
  WalletPlaceholderMessageZap,
  WalletNwcRequest,
  WalletProfileZapDraft,
  WalletProfileZapRequest,
  WalletProfileZapResult,
  WalletRecipientOffer,
  WalletSendRequest,
  WalletStatus,
  WalletTransactionPage,
} from "./types";
import type { RelayEvent } from "@/shared/api/types";

let compileEnabledPromise: Promise<boolean> | null = null;

function communityRelayUrls(): string[] {
  return loadCommunities().map((community) => community.relayUrl);
}

export function bitcoinCompileEnabled(): Promise<boolean> {
  compileEnabledPromise ??= invoke<boolean>("bitcoin_compile_enabled");
  return compileEnabledPromise;
}

export function enableWallet(): Promise<WalletEnableResult> {
  return invoke<WalletEnableResult>("wallet_enable", {
    relayUrls: communityRelayUrls(),
  });
}

export function disableWallet(): Promise<WalletOfferPublicationResult> {
  return invoke<WalletOfferPublicationResult>("wallet_disable", {
    relayUrls: communityRelayUrls(),
  });
}

export function getWalletStatus(): Promise<WalletStatus> {
  return invoke<WalletStatus>("wallet_get_status");
}

export function createWalletReceiveRequest(): Promise<WalletFundingRequest> {
  return invoke<WalletFundingRequest>("wallet_create_receive_request");
}

export function refreshWalletOffer(): Promise<WalletOfferPublicationResult> {
  return invoke<WalletOfferPublicationResult>("wallet_refresh_offer", {
    relayUrls: communityRelayUrls(),
  });
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

export function parseNwcWalletRequest(
  event: RelayEvent,
): Promise<WalletNwcRequest> {
  return invoke<WalletNwcRequest>("wallet_parse_nwc_request", { event });
}

export function buildNwcWalletResponse(input: {
  event: RelayEvent;
  payment?: WalletPaymentResult | null;
  errorCode?: string | null;
  errorMessage?: string | null;
}): Promise<RelayEvent> {
  return invoke<RelayEvent>("wallet_build_nwc_response", {
    event: input.event,
    payment: input.payment ?? null,
    errorCode: input.errorCode ?? null,
    errorMessage: input.errorMessage ?? null,
  });
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
  targetEventId?: string | null,
): Promise<WalletProfileZapDraft | null> {
  return invoke<WalletProfileZapDraft | null>(
    "wallet_get_pending_profile_zap",
    {
      recipientPubkey,
      targetEventId: targetEventId ?? null,
    },
  );
}

export function listPlaceholderMessageZaps(): Promise<
  WalletPlaceholderMessageZap[]
> {
  return invoke<WalletPlaceholderMessageZap[]>(
    "wallet_list_placeholder_message_zaps",
  );
}

export function sendProfileZap(
  request: WalletProfileZapRequest,
): Promise<WalletProfileZapResult> {
  return invoke<WalletProfileZapResult>("wallet_send_profile_zap", {
    request,
  });
}

export function sendAgentRuntimeZap(
  request: WalletAgentRuntimeZapRequest,
): Promise<WalletProfileZapResult> {
  return invoke<WalletProfileZapResult>("wallet_send_agent_runtime_zap", {
    request,
  });
}

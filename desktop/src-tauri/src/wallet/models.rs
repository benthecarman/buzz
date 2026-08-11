use serde::{Deserialize, Serialize};

/// Provider-neutral snapshot displayed at the top of wallet settings.
///
/// All amounts are whole satoshis. `balance` is the provider's total wallet
/// balance, while `spendable_balance` is the amount currently sendable over
/// Lightning after channel reserves and other provider constraints.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WalletStatus {
    pub provider_name: String,
    pub balance: u64,
    pub spendable_balance: u64,
    pub lightning_balance: u64,
    pub onchain_balance: u64,
}

/// Wallet status returned after first-time provisioning and offer publication.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WalletEnableResult {
    pub status: WalletStatus,
    pub publication_warnings: Vec<String>,
}

/// Result of publishing or withdrawing a replaceable wallet offer.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WalletOfferPublicationResult {
    pub offer: Option<String>,
    pub publication_warnings: Vec<String>,
}

/// Amountless receive request containing both Lightning payment rails.
///
/// `bip321_uri` is the value rendered as a QR code. It embeds the temporary,
/// amountless BOLT11 invoice and the wallet's reusable BOLT12 offer, allowing
/// the funding wallet to choose an amount and the rail it supports.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WalletFundingRequest {
    pub bip321_uri: String,
    pub bolt11_invoice: String,
    pub bolt11_expires_at_ms: u64,
    pub bolt12_offer: String,
}

/// Provider-neutral interpretation of a user-entered payment destination.
///
/// The provider may resolve several formats (for example BOLT11, BOLT12,
/// BIP-321, or a Lightning address). Amount fields are whole satoshis; a
/// missing `amount` means the caller must supply one within the optional bounds.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WalletDestinationAnalysis {
    pub normalized_destination: String,
    pub description: Option<String>,
    pub amount: Option<u64>,
    pub min_amount: Option<u64>,
    pub max_amount: Option<u64>,
    pub expires_at_ms: Option<u64>,
}

/// A request to send Bitcoin through the selected provider.
///
/// `request_id` is a client-generated UUID that makes `wallet_send`
/// idempotent: repeat invokes with the same ID reconcile the recorded attempt
/// instead of sending again. It is also recorded in the provider's private
/// payment note for reconciliation.
#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WalletSendRequest {
    pub destination: String,
    pub amount: Option<u64>,
    pub message: Option<String>,
    pub request_id: String,
}

/// A provider-neutral request to pay a raw BOLT12 offer.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WalletOfferSendRequest {
    pub offer: String,
    pub amount: u64,
    pub payer_note: String,
    pub personal_note: String,
}

/// Terminal or current state of one provider payment.
///
/// `payment_id` is opaque provider state serialized for display and recovery;
/// callers must not parse it. Amounts and fees are whole satoshis, and
/// timestamps are Unix milliseconds.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WalletPaymentResult {
    pub payment_id: String,
    pub status: String,
    pub status_message: String,
    pub amount: Option<u64>,
    pub fees: u64,
    pub created_at_ms: u64,
    pub finalized_at_ms: Option<u64>,
}

/// One provider-neutral wallet history entry.
///
/// `direction` and `status` are stable string forms of provider states. Amounts
/// and fees are whole satoshis, and timestamps are Unix milliseconds.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WalletTransaction {
    pub id: String,
    pub direction: String,
    pub status: String,
    pub status_message: String,
    pub amount: Option<u64>,
    pub fees: u64,
    pub note: Option<String>,
    /// Public payer metadata supplied with an inbound BOLT12 payment.
    pub payer_note: Option<String>,
    /// Provider-canonical identifier of the BOLT12 offer used by the payment.
    pub offer_id: Option<String>,
    pub created_at_ms: u64,
    pub finalized_at_ms: Option<u64>,
}

/// A newest-first page of wallet history.
///
/// `next_cursor` is opaque and may only be passed back to
/// `wallet_list_transactions`; it is not an event or payment identifier.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WalletTransactionPage {
    pub transactions: Vec<WalletTransaction>,
    pub next_cursor: Option<String>,
}

/// Recipient-authored offer and the signed event that authorized it.
///
/// The complete event JSON is retained because the BOLT12 zap intent embeds
/// that exact signed event; an offer string alone is insufficient to prove
/// that the recipient advertised it.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WalletRecipientOffer {
    pub recipient_pubkey: String,
    pub offer: String,
    pub offer_event_json: String,
    pub offer_event_id: String,
}

/// A signed NIP-B1 zap whose embedded offer was parsed by rust-lightning.
///
/// Amounts are whole satoshis. Renderer code must use this native result
/// instead of interpreting BOLT12 strings itself.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WalletVerifiedZapEvent {
    pub event_id: String,
    pub amount: u64,
    pub comment: String,
    pub intent_event_id: String,
    pub recipient_pubkey: String,
    pub target_event_id: Option<String>,
    pub channel_id: Option<String>,
}

/// A request to send an attributed profile or event zap.
#[derive(Clone, Debug, Eq, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WalletProfileZapRequest {
    pub recipient_pubkey: String,
    pub amount: u64,
    pub comment: Option<String>,
    pub idempotency_key: String,
    /// Event id for an event-targeted zap. Omitted for profile zaps.
    #[serde(default)]
    pub target_event_id: Option<String>,
    /// Kind of the target event. Required when `target_event_id` is present.
    #[serde(default)]
    pub target_event_kind: Option<u32>,
}

/// Purchase of one invocation window against the Agent's published terms.
#[derive(Clone, Debug, Eq, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WalletAgentRuntimeZapRequest {
    /// Agent whose published terms this purchase pays against.
    pub agent_pubkey: String,
    /// Non-DM channel in which the zap grants access.
    pub channel_id: String,
    /// Exact signed kind-10101 pricing event that the zap targets.
    pub pricing_event_json: String,
    /// Client-generated UUID reused for every retry of this exact purchase.
    pub idempotency_key: String,
}

/// Restorable UI fields for an incomplete profile payment.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WalletProfileZapDraft {
    pub recipient_pubkey: String,
    pub amount: u64,
    pub comment: Option<String>,
    pub idempotency_key: String,
    pub target_event_id: Option<String>,
    pub target_event_kind: Option<u32>,
}

/// Result of the experimental profile-payment flow.
///
/// The current provider publishes a temporary placeholder receipt after local
/// settlement; `proof_published` reports whether the relay accepted it.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WalletProfileZapResult {
    pub payment: WalletPaymentResult,
    pub intent_event_id: String,
    pub proof_event_id: Option<String>,
    pub proof_published: bool,
}

/// Validated agent-originated NWC-321 payment request shown for approval.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WalletNwcRequest {
    pub event_id: String,
    pub agent_pubkey: String,
    pub agent_name: String,
    pub recipient_pubkey: String,
    pub amount: u64,
    pub comment: String,
    pub destination: String,
    pub payer_note: String,
    pub request_id: String,
}

/// A stable, serializable wallet error returned through Tauri.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct WalletError {
    pub code: &'static str,
    pub message: String,
}

impl WalletError {
    pub fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }

    pub fn unavailable(message: impl Into<String>) -> Self {
        Self::new("wallet_unavailable", message)
    }

    pub fn provider(message: impl Into<String>) -> Self {
        Self::new("provider_error", message)
    }
}

impl std::fmt::Display for WalletError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}

impl std::error::Error for WalletError {}

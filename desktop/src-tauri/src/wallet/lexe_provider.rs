use std::{path::Path, str::FromStr, sync::Arc};

use async_trait::async_trait;
use lexe::{
    config::WalletEnvConfig,
    types::{
        auth::RootSeed,
        bitcoin::Amount,
        command::{
            AnalyzeRequest, CreateInvoiceRequest, CreateOfferRequest, PayOfferRequest, PayRequest,
            PaymentSyncSummary,
        },
        payment::{Order, Payment, PaymentCreatedIndex, PaymentDirection, PaymentFilter},
    },
    wallet::LexeWallet,
};
use sha2_10::{Digest, Sha256};
use tokio::sync::Mutex;

use super::{
    models::{
        WalletDestinationAnalysis, WalletError, WalletFundingRequest, WalletOfferSendRequest,
        WalletPaymentResult, WalletSendRequest, WalletStatus, WalletTransaction,
        WalletTransactionPage,
    },
    provider::{bip321_uri, WalletPaymentMatch, WalletProvider},
    seed::WalletSeed,
};

const OFFER_EXPIRATION_SECS: u32 = 60 * 60 * 24 * 365 * 50;
// Lexe limits BOLT11 invoices to one day. The accompanying BOLT12 offer
// remains the long-lived funding method in the BIP-321 request.
const FUNDING_INVOICE_EXPIRATION_SECS: u32 = 60 * 60 * 24;

fn payment_sync_changed(summary: &PaymentSyncSummary) -> bool {
    summary.num_new > 0 || summary.num_updated > 0
}

fn reconciliation_fields_match(
    direction: PaymentDirection,
    payer_note: Option<&str>,
    personal_note: Option<&str>,
    amount: Option<u64>,
    offer_id: Option<&lexe::types::payment::OfferId>,
    payment_match: &WalletPaymentMatch<'_>,
    expected_offer_id: Option<&lexe::types::payment::OfferId>,
) -> bool {
    direction == PaymentDirection::Outbound
        && payment_match
            .payer_note
            .is_none_or(|note| payer_note == Some(note))
        && payment_match
            .personal_note
            .is_none_or(|note| personal_note == Some(note))
        && payment_match
            .expected_amount
            .is_none_or(|expected| amount == Some(expected))
        && expected_offer_id.is_none_or(|expected| offer_id == Some(expected))
}

pub(super) fn canonical_offer(value: &str) -> bool {
    lexe::types::bitcoin::Offer::from_str(value)
        .map(|offer| offer.to_string() == value)
        .unwrap_or(false)
}

fn scoped_offer_file_name(scope: &str) -> String {
    let digest = Sha256::digest(scope.as_bytes());
    format!("{}.txt", hex::encode(digest))
}

/// Creates the Lexe adapter for one identity-scoped wallet cache.
pub(super) fn create_lexe_provider(
    seed: WalletSeed,
    cache_dir: &Path,
) -> Result<Arc<dyn WalletProvider>, WalletError> {
    std::fs::create_dir_all(cache_dir)
        .map_err(|error| WalletError::unavailable(format!("create wallet cache: {error}")))?;

    let root_seed = RootSeed::from_bytes(seed.as_bytes())
        .map_err(|error| WalletError::unavailable(format!("create Lexe seed: {error}")))?;
    let wallet = LexeWallet::load_or_fresh(
        WalletEnvConfig::mainnet(),
        (&root_seed).into(),
        Some(cache_dir.to_path_buf()),
    )
    .map_err(|error| WalletError::provider(format!("initialize Lexe wallet: {error:#}")))?;

    Ok(Arc::new(LexeProvider {
        wallet,
        root_seed,
        offer_path: cache_dir.join("active-offer.txt"),
        scoped_offer_dir: cache_dir.join("offers"),
        offer_lock: Mutex::new(()),
    }))
}

/// Lexe SDK adapter for one deterministic Buzz wallet.
///
/// This is the only wallet module allowed to depend on Lexe SDK types. It
/// normalizes balances, payment states, cursors, and errors before returning
/// them through `WalletProvider`.
struct LexeProvider {
    /// Lexe client bound to this Nostr identity's deterministic root seed.
    wallet: LexeWallet,
    /// Retained because Lexe's idempotent signup API requires the root seed.
    root_seed: RootSeed,
    /// Disk location of the persisted active offer. Lexe 0.1.20 has no API to
    /// recover an existing offer and `create_offer` never invalidates prior
    /// ones, so the offer is persisted here to keep a restart from minting a
    /// fresh one and orphaning the previously published offer.
    offer_path: std::path::PathBuf,
    /// Directory of stable, recipient-scoped offers. File names are hashes of
    /// opaque scopes, so a caller cannot escape the wallet cache directory.
    scoped_offer_dir: std::path::PathBuf,
    /// Serializes read-create-persist so concurrent requests for one missing
    /// scope cannot mint two offers and publish different values.
    offer_lock: Mutex<()>,
}

impl LexeProvider {
    fn scoped_offer_path(&self, scope: &str) -> std::path::PathBuf {
        self.scoped_offer_dir.join(scoped_offer_file_name(scope))
    }

    async fn offer_at(
        &self,
        path: &Path,
        description: &str,
        rotate: bool,
    ) -> Result<String, WalletError> {
        let _guard = self.offer_lock.lock().await;
        if !rotate {
            if let Some(offer) = std::fs::read_to_string(path)
                .ok()
                .map(|offer| offer.trim().to_string())
                .filter(|offer| canonical_offer(offer))
            {
                return Ok(offer);
            }
        }
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|error| {
                WalletError::unavailable(format!("create wallet offer directory: {error}"))
            })?;
        }
        let offer = self
            .wallet
            .create_offer(CreateOfferRequest {
                description: Some(description.to_string()),
                expiration_secs: Some(OFFER_EXPIRATION_SECS),
                ..Default::default()
            })
            .await
            .map_err(|error| WalletError::provider(format!("create BOLT12 offer: {error:#}")))?
            .offer
            .to_string();
        // The offer is public and published to Nostr. A persistence failure is
        // non-fatal, but the next process can mint a replacement for the scope.
        if let Err(error) = std::fs::write(path, &offer) {
            tracing::warn!(error = %error, "persist wallet offer");
        }
        Ok(offer)
    }

    fn amount(value: u64) -> Result<Amount, WalletError> {
        Amount::try_from_sats_u64(value)
            .map_err(|error| WalletError::new("invalid_amount", error.to_string()))
    }

    fn json_string<T: serde::Serialize>(value: &T) -> String {
        serde_json::to_value(value)
            .ok()
            .and_then(|value| value.as_str().map(str::to_owned))
            .unwrap_or_else(|| "unknown".to_string())
    }

    fn payment_id(index: &PaymentCreatedIndex) -> Result<String, WalletError> {
        serde_json::to_string(index)
            .map_err(|error| WalletError::provider(format!("encode payment id: {error}")))
    }

    fn payment_result(payment: Payment) -> Result<WalletPaymentResult, WalletError> {
        Ok(WalletPaymentResult {
            payment_id: Self::payment_id(&payment.index)?,
            status: Self::json_string(&payment.status),
            status_message: payment.status_msg,
            amount: payment.amount.map(|amount| amount.sats_u64()),
            fees: payment.fees.sats_u64(),
            created_at_ms: payment.created_at.to_millis(),
            finalized_at_ms: payment.finalized_at.map(|timestamp| timestamp.to_millis()),
        })
    }

    fn transaction(payment: Payment) -> Result<WalletTransaction, WalletError> {
        Ok(WalletTransaction {
            id: Self::payment_id(&payment.index)?,
            direction: Self::json_string(&payment.direction),
            status: Self::json_string(&payment.status),
            status_message: payment.status_msg,
            amount: payment.amount.map(|amount| amount.sats_u64()),
            fees: payment.fees.sats_u64(),
            note: payment
                .personal_note
                .or(payment.message)
                .or(payment.payer_name),
            created_at_ms: payment.created_at.to_millis(),
            finalized_at_ms: payment.finalized_at.map(|timestamp| timestamp.to_millis()),
        })
    }
}

#[async_trait]
impl WalletProvider for LexeProvider {
    async fn provision(&self) -> Result<(), WalletError> {
        // Lexe documents signup as idempotent. It also performs initial
        // provisioning, so this handles both first enable and recovery on a
        // clean install using the same deterministic root seed.
        self.wallet
            .signup(&self.root_seed, None)
            .await
            .map_err(|error| WalletError::provider(format!("provision Lexe wallet: {error:#}")))
    }

    async fn status(&self) -> Result<WalletStatus, WalletError> {
        let info = self
            .wallet
            .node_info()
            .await
            .map_err(|error| WalletError::provider(format!("load Lexe balance: {error:#}")))?;
        Ok(WalletStatus {
            provider_name: "Lexe".to_string(),
            balance: info.balance.sats_u64(),
            spendable_balance: info.lightning_sendable_balance.sats_u64(),
            lightning_balance: info.lightning_balance.sats_u64(),
            onchain_balance: info.onchain_balance.sats_u64(),
        })
    }

    async fn offer(&self, rotate: bool) -> Result<String, WalletError> {
        self.offer_at(&self.offer_path, "Buzz wallet", rotate).await
    }

    async fn scoped_offer(&self, scope: String, rotate: bool) -> Result<String, WalletError> {
        if scope.is_empty() {
            return Err(WalletError::new(
                "invalid_offer_scope",
                "wallet offer scope must not be empty",
            ));
        }
        let path = self.scoped_offer_path(&scope);
        self.offer_at(&path, "Buzz agent", rotate).await
    }

    async fn funding_request(&self) -> Result<WalletFundingRequest, WalletError> {
        let invoice = self
            .wallet
            .create_invoice(CreateInvoiceRequest {
                expiration_secs: Some(FUNDING_INVOICE_EXPIRATION_SECS),
                amount: None,
                description: Some("Fund Buzz wallet".to_string()),
                ..Default::default()
            })
            .await
            .map_err(|error| WalletError::provider(format!("create BOLT11 invoice: {error:#}")))?;

        let offer = self.offer(false).await?;

        let bolt11_invoice = invoice.invoice.to_string();
        let bip321_uri = bip321_uri(None, Some(&bolt11_invoice), Some(&offer))?;
        Ok(WalletFundingRequest {
            bip321_uri,
            bolt11_invoice,
            bolt11_expires_at_ms: invoice.expires_at.to_millis(),
            bolt12_offer: offer,
        })
    }

    async fn analyze(&self, destination: String) -> Result<WalletDestinationAnalysis, WalletError> {
        let response = self
            .wallet
            .analyze(AnalyzeRequest {
                payment_string: destination,
            })
            .await
            .map_err(|error| {
                WalletError::new(
                    "invalid_destination",
                    format!("analyze destination: {error:#}"),
                )
            })?;
        let payable = response.payables.into_iter().next().ok_or_else(|| {
            WalletError::new("invalid_destination", "no payable destination was found")
        })?;
        Ok(WalletDestinationAnalysis {
            normalized_destination: payable.payable,
            description: payable.description,
            amount: payable.amount.map(|amount| amount.sats_u64()),
            min_amount: payable.min_amount.map(|amount| amount.sats_u64()),
            max_amount: payable.max_amount.map(|amount| amount.sats_u64()),
            expires_at_ms: payable.expires_at.map(|timestamp| timestamp.to_millis()),
        })
    }

    async fn send(&self, request: WalletSendRequest) -> Result<WalletPaymentResult, WalletError> {
        let amount = request.amount.map(Self::amount).transpose()?;
        let payment = self
            .wallet
            .pay(PayRequest {
                payable: request.destination,
                amount,
                message: request.message,
                personal_note: Some(format!("Buzz payment {}", request.request_id)),
            })
            .await
            .map_err(|error| WalletError::new("payment_failed", format!("{error:#}")))?;
        Self::payment_result(payment)
    }

    async fn send_offer(
        &self,
        request: WalletOfferSendRequest,
    ) -> Result<WalletPaymentResult, WalletError> {
        let offer = lexe::types::bitcoin::Offer::from_str(&request.offer)
            .map_err(|error| WalletError::new("invalid_destination", error.to_string()))?;
        let payment = self
            .wallet
            .pay_offer(PayOfferRequest {
                offer,
                amount: Self::amount(request.amount)?,
                message: Some(request.payer_note),
                personal_note: Some(request.personal_note),
            })
            .await
            .map_err(|error| WalletError::new("payment_failed", format!("{error:#}")))?;
        Self::payment_result(payment)
    }

    async fn find_outbound_payment(
        &self,
        payment_match: WalletPaymentMatch<'_>,
    ) -> Result<Option<WalletPaymentResult>, WalletError> {
        let expected_offer_id = payment_match
            .expected_offer
            .map(lexe::types::bitcoin::Offer::from_str)
            .transpose()
            .map_err(|error| WalletError::new("offer_invalid", error.to_string()))?
            .map(|offer| offer.id());
        self.wallet
            .sync_payments()
            .await
            .map_err(|error| WalletError::provider(format!("sync payments: {error:#}")))?;
        let mut after = None;
        loop {
            let page = self
                .wallet
                .list_payments(
                    &PaymentFilter::All,
                    Some(Order::Desc),
                    Some(100),
                    after.as_ref(),
                )
                .map_err(|error| WalletError::provider(format!("list payments: {error:#}")))?;
            if let Some(payment) = page.payments.into_iter().find(|payment| {
                reconciliation_fields_match(
                    payment.direction,
                    payment.message.as_deref(),
                    payment.personal_note.as_deref(),
                    payment.amount.map(|value| value.sats_u64()),
                    payment.offer_id.as_ref(),
                    &payment_match,
                    expected_offer_id.as_ref(),
                )
            }) {
                return Self::payment_result(payment).map(Some);
            }
            let Some(next_index) = page.next_index else {
                return Ok(None);
            };
            after = Some(next_index);
        }
    }

    async fn poll_updates(&self) -> Result<bool, WalletError> {
        self.wallet
            .sync_payments()
            .await
            .map(|summary| payment_sync_changed(&summary))
            .map_err(|error| WalletError::provider(format!("sync payments: {error:#}")))
    }

    async fn transactions(
        &self,
        cursor: Option<String>,
        limit: usize,
        sync: bool,
    ) -> Result<WalletTransactionPage, WalletError> {
        if sync {
            self.wallet
                .sync_payments()
                .await
                .map_err(|error| WalletError::provider(format!("sync payments: {error:#}")))?;
        }
        let after = cursor
            .as_deref()
            .map(serde_json::from_str::<PaymentCreatedIndex>)
            .transpose()
            .map_err(|error| WalletError::new("invalid_cursor", error.to_string()))?;
        let page = self
            .wallet
            .list_payments(
                &PaymentFilter::All,
                Some(Order::Desc),
                Some(limit.clamp(1, 100)),
                after.as_ref(),
            )
            .map_err(|error| WalletError::provider(format!("list payments: {error:#}")))?;
        let transactions = page
            .payments
            .into_iter()
            .map(Self::transaction)
            .collect::<Result<Vec<_>, _>>()?;
        let next_cursor = page.next_index.as_ref().map(Self::payment_id).transpose()?;
        Ok(WalletTransactionPage {
            transactions,
            next_cursor,
        })
    }
}

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use lexe::types::{
        bitcoin::{Invoice, Offer},
        command::PaymentSyncSummary,
        payment::PaymentDirection,
    };

    use super::{
        payment_sync_changed, reconciliation_fields_match, scoped_offer_file_name,
        WalletPaymentMatch,
    };
    use crate::wallet::{VALID_INVOICE, VALID_OFFER};

    const VALID_PAYER_NOTE: &str =
        "nostr:nipB1:c63e8667c29f5db1dbdec9ce4d720b692a15665c03530af5a978701783a073bb";

    #[test]
    fn payment_fixtures_are_canonical() {
        assert_eq!(
            Invoice::from_str(VALID_INVOICE).unwrap().to_string(),
            VALID_INVOICE
        );
        assert_eq!(
            Offer::from_str(VALID_OFFER).unwrap().to_string(),
            VALID_OFFER
        );
    }

    #[test]
    fn scoped_offer_files_are_stable_distinct_and_path_safe() {
        let first = scoped_offer_file_name("agent-a");
        assert_eq!(first, scoped_offer_file_name("agent-a"));
        assert_ne!(first, scoped_offer_file_name("agent-b"));
        assert!(!first.contains('/'));
        assert!(!scoped_offer_file_name("../agent").contains(".."));
    }

    #[test]
    fn payment_sync_only_reports_actual_changes() {
        assert!(!payment_sync_changed(&PaymentSyncSummary {
            num_new: 0,
            num_updated: 0,
        }));
        assert!(payment_sync_changed(&PaymentSyncSummary {
            num_new: 1,
            num_updated: 0,
        }));
        assert!(payment_sync_changed(&PaymentSyncSummary {
            num_new: 0,
            num_updated: 1,
        }));
    }

    #[test]
    fn reconciliation_requires_matching_outbound_payment_fields() {
        let offer_id = Offer::from_str(VALID_OFFER).unwrap().id();
        let expected = WalletPaymentMatch {
            payer_note: Some(VALID_PAYER_NOTE),
            personal_note: Some("Buzz profile payment intent"),
            expected_amount: Some(21),
            expected_offer: Some(VALID_OFFER),
        };
        let matches = |direction, payer_note, amount, actual_offer_id| {
            reconciliation_fields_match(
                direction,
                payer_note,
                Some("Buzz profile payment intent"),
                amount,
                actual_offer_id,
                &expected,
                Some(&offer_id),
            )
        };
        assert!(matches(
            PaymentDirection::Outbound,
            Some(VALID_PAYER_NOTE),
            Some(21),
            Some(&offer_id)
        ));
        assert!(!matches(
            PaymentDirection::Inbound,
            Some(VALID_PAYER_NOTE),
            Some(21),
            Some(&offer_id)
        ));
        assert!(!matches(
            PaymentDirection::Outbound,
            Some(VALID_PAYER_NOTE),
            Some(22),
            Some(&offer_id)
        ));
        assert!(!matches(
            PaymentDirection::Outbound,
            Some("another-note"),
            Some(21),
            Some(&offer_id)
        ));
        assert!(!matches(
            PaymentDirection::Outbound,
            Some(VALID_PAYER_NOTE),
            Some(21),
            None
        ));
    }
}

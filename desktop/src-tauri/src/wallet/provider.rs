use async_trait::async_trait;

use super::models::{
    WalletDestinationAnalysis, WalletError, WalletFundingRequest, WalletOfferSendRequest,
    WalletPaymentResult, WalletSendRequest, WalletStatus, WalletTransactionPage,
};

pub(crate) struct WalletPaymentMatch<'a> {
    pub payer_note: Option<&'a str>,
    pub personal_note: Option<&'a str>,
    pub expected_amount: Option<u64>,
    pub expected_offer: Option<&'a str>,
}

/// Provider-neutral operations needed by the current wallet UI.
///
/// Implementations translate their SDK types into the DTOs in `models`; no
/// provider-specific type should cross this boundary. Keep this trait limited
/// to operations exercised by Buzz rather than anticipated provider features.
#[async_trait]
pub trait WalletProvider: Send + Sync {
    /// Register a new wallet and complete its initial provisioning.
    async fn signup(&self) -> Result<(), WalletError>;

    /// Provision all current provider releases for an existing wallet.
    async fn provision(&self) -> Result<(), WalletError>;

    async fn status(&self) -> Result<WalletStatus, WalletError>;

    async fn offer(&self, rotate: bool) -> Result<String, WalletError>;

    async fn funding_request(&self) -> Result<WalletFundingRequest, WalletError>;

    async fn analyze(&self, destination: String) -> Result<WalletDestinationAnalysis, WalletError>;

    async fn send(&self, request: WalletSendRequest) -> Result<WalletPaymentResult, WalletError>;

    async fn send_offer(
        &self,
        request: WalletOfferSendRequest,
    ) -> Result<WalletPaymentResult, WalletError>;

    async fn find_outbound_payment(
        &self,
        payment_match: WalletPaymentMatch<'_>,
    ) -> Result<Option<WalletPaymentResult>, WalletError>;

    async fn poll_updates(&self) -> Result<bool, WalletError>;

    async fn transactions(
        &self,
        cursor: Option<String>,
        limit: usize,
        sync: bool,
    ) -> Result<WalletTransactionPage, WalletError>;
}

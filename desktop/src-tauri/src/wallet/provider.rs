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

fn bitcoin_amount(amount: u64) -> String {
    let whole = amount / 100_000_000;
    let fractional = amount % 100_000_000;
    if fractional == 0 {
        return whole.to_string();
    }

    let fractional = format!("{fractional:08}");
    format!("{whole}.{}", fractional.trim_end_matches('0'))
}

pub(super) fn bip321_uri(
    amount: Option<u64>,
    bolt11_invoice: Option<&str>,
    bolt12_offer: Option<&str>,
) -> Result<String, WalletError> {
    if amount == Some(0) {
        return Err(WalletError::new(
            "invalid_amount",
            "Bitcoin amount must be greater than zero",
        ));
    }

    let bolt11_invoice = bolt11_invoice.filter(|value| !value.is_empty());
    let bolt12_offer = bolt12_offer.filter(|value| !value.is_empty());
    if bolt11_invoice.is_none() && bolt12_offer.is_none() {
        return Err(WalletError::new(
            "invalid_funding_request",
            "at least one Lightning payment method is required",
        ));
    }

    let mut query = url::form_urlencoded::Serializer::new(String::new());
    if let Some(amount) = amount {
        query.append_pair("amount", &bitcoin_amount(amount));
    }
    if let Some(invoice) = bolt11_invoice {
        query.append_pair("lightning", invoice);
    }
    if let Some(offer) = bolt12_offer {
        query.append_pair("lno", offer);
    }
    Ok(format!("bitcoin:?{}", query.finish()))
}

/// Provider-neutral operations needed by the current wallet UI.
///
/// Implementations translate their SDK types into the DTOs in `models`; no
/// provider-specific type should cross this boundary. Keep this trait limited
/// to operations exercised by Buzz rather than anticipated provider features.
#[async_trait]
pub trait WalletProvider: Send + Sync {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wallet::{VALID_INVOICE, VALID_OFFER};

    fn query_pairs(uri: &str) -> std::collections::HashMap<String, String> {
        url::Url::parse(uri)
            .unwrap()
            .query_pairs()
            .map(|(key, value)| (key.into_owned(), value.into_owned()))
            .collect()
    }

    #[test]
    fn bip321_uri_supports_both_lightning_methods() {
        let uri = bip321_uri(Some(123_456_789), Some(VALID_INVOICE), Some(VALID_OFFER)).unwrap();
        let pairs = query_pairs(&uri);
        assert_eq!(pairs.get("amount").unwrap(), "1.23456789");
        assert_eq!(pairs.get("lightning").unwrap(), VALID_INVOICE);
        assert_eq!(pairs.get("lno").unwrap(), VALID_OFFER);
    }

    #[test]
    fn bip321_uri_supports_either_lightning_method() {
        let invoice_only = query_pairs(&bip321_uri(Some(100), Some(VALID_INVOICE), None).unwrap());
        assert_eq!(invoice_only.get("amount").unwrap(), "0.000001");
        assert_eq!(invoice_only.get("lightning").unwrap(), VALID_INVOICE);
        assert!(!invoice_only.contains_key("lno"));

        let offer_only =
            query_pairs(&bip321_uri(Some(100_000_000), None, Some(VALID_OFFER)).unwrap());
        assert_eq!(offer_only.get("amount").unwrap(), "1");
        assert_eq!(offer_only.get("lno").unwrap(), VALID_OFFER);
        assert!(!offer_only.contains_key("lightning"));
    }

    #[test]
    fn bip321_uri_supports_amountless_funding() {
        let pairs = query_pairs(&bip321_uri(None, Some(VALID_INVOICE), Some(VALID_OFFER)).unwrap());
        assert!(!pairs.contains_key("amount"));
        assert_eq!(pairs.get("lightning").unwrap(), VALID_INVOICE);
        assert_eq!(pairs.get("lno").unwrap(), VALID_OFFER);
    }

    #[test]
    fn bip321_uri_rejects_explicit_zero_and_no_payment_methods() {
        assert_eq!(
            bip321_uri(Some(0), Some(VALID_INVOICE), Some(VALID_OFFER))
                .unwrap_err()
                .code,
            "invalid_amount"
        );
        assert_eq!(
            bip321_uri(None, None, None).unwrap_err().code,
            "invalid_funding_request"
        );
    }
}

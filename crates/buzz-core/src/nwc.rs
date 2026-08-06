//! NIP-47 wire helpers for the agent-to-owner wallet boundary.
//!
//! This module only builds, validates, encrypts, and decrypts events. It does
//! not authorize an agent or dispatch a wallet payment.

use nostr::{
    nips::nip44::{self, Version},
    Event, EventBuilder, Keys, Kind, PublicKey, Tag,
};
use serde::{Deserialize, Serialize};

use crate::kind::{KIND_NWC_REQUEST, KIND_NWC_RESPONSE};

/// NWC-321 `pay` parameters used for a BOLT12 zap payment.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct NwcPayParams {
    /// BIP-321 URI containing the selected Lightning instruction.
    pub payment: String,
    /// Amount in millisatoshis. Required for amountless BOLT12 offers.
    pub amount: u64,
    /// NIP-B1 payer note binding the payment to the signed zap intent.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub payer_note: Option<String>,
    /// Optional application metadata.
    #[serde(default, skip_serializing_if = "serde_json::Map::is_empty")]
    pub metadata: serde_json::Map<String, serde_json::Value>,
}

/// Decrypted NWC-321 `pay` request body.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct NwcPayRequest {
    /// Always `pay` for this request type.
    pub method: String,
    /// Payment parameters.
    pub params: NwcPayParams,
}

/// NWC error returned by the wallet service.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct NwcErrorBody {
    /// Stable NWC error code.
    pub code: String,
    /// Human-readable failure explanation.
    pub message: String,
}

/// NWC-321 payment result.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct NwcPayResult {
    /// Wallet-scoped transaction identifier.
    pub transaction_id: String,
    /// `pending`, `settled`, or `failed`.
    pub state: String,
    /// `bolt11` or `bolt12`.
    pub instruction_type: String,
    /// Paid amount in millisatoshis.
    pub amount: u64,
    /// Fees in millisatoshis, when known.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fees_paid: Option<u64>,
    /// BOLT12 payer proof, when exposed by the wallet provider.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub payer_proof: Option<String>,
    /// Failure detail. Required by NWC-321 when `state` is `failed`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub failure_reason: Option<String>,
    /// Transaction creation time as Unix seconds.
    pub created_at: u64,
    /// Settlement time as Unix seconds. Required when `state` is `settled`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub settled_at: Option<u64>,
}

/// Decrypted NWC response body for the `pay` method.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct NwcPayResponse {
    /// Always `pay` for this response type.
    pub result_type: String,
    /// Non-null when the request failed or was denied.
    pub error: Option<NwcErrorBody>,
    /// Present when the wallet accepted or completed the payment.
    pub result: Option<NwcPayResult>,
}

/// NIP-47 event validation or encryption failure.
#[derive(Debug, thiserror::Error)]
pub enum NwcWireError {
    /// The signed event or its tags do not match NIP-47.
    #[error("invalid NWC event: {0}")]
    Invalid(String),
    /// NIP-44 encryption or decryption failed.
    #[error("NWC encryption failed: {0}")]
    Encryption(String),
    /// The decrypted JSON body is invalid.
    #[error("invalid NWC payload: {0}")]
    Payload(String),
}

fn tag(parts: impl IntoIterator<Item = impl Into<String>>) -> Result<Tag, NwcWireError> {
    Tag::parse(parts).map_err(|error| NwcWireError::Invalid(error.to_string()))
}

fn validate_recipient(
    event: &Event,
    expected_kind: u32,
    recipient: &PublicKey,
) -> Result<(), NwcWireError> {
    event
        .verify()
        .map_err(|error| NwcWireError::Invalid(error.to_string()))?;
    if event.kind != Kind::Custom(expected_kind as u16) {
        return Err(NwcWireError::Invalid("unexpected event kind".into()));
    }
    let recipients = event
        .tags
        .iter()
        .filter_map(|tag| {
            let parts = tag.as_slice();
            (parts.first().map(String::as_str) == Some("p"))
                .then(|| parts.get(1))
                .flatten()
        })
        .collect::<Vec<_>>();
    if recipients.len() != 1 || recipients[0].as_str() != recipient.to_hex() {
        return Err(NwcWireError::Invalid(
            "event must contain exactly one matching p tag".into(),
        ));
    }
    if !event
        .tags
        .iter()
        .any(|tag| tag.as_slice() == ["encryption".to_string(), "nip44_v2".to_string()])
    {
        return Err(NwcWireError::Invalid(
            "event must declare nip44_v2 encryption".into(),
        ));
    }
    Ok(())
}

/// Build an unsigned encrypted kind-23194 NWC-321 `pay` request.
pub fn build_pay_request(
    client: &Keys,
    wallet_service: &PublicKey,
    params: NwcPayParams,
    expiration: u64,
) -> Result<EventBuilder, NwcWireError> {
    if params.amount == 0 || params.payment.is_empty() {
        return Err(NwcWireError::Invalid(
            "payment and a positive amount are required".into(),
        ));
    }
    let plaintext = serde_json::to_string(&NwcPayRequest {
        method: "pay".into(),
        params,
    })
    .map_err(|error| NwcWireError::Payload(error.to_string()))?;
    let ciphertext = nip44::encrypt(client.secret_key(), wallet_service, plaintext, Version::V2)
        .map_err(|error| NwcWireError::Encryption(error.to_string()))?;
    Ok(
        EventBuilder::new(Kind::Custom(KIND_NWC_REQUEST as u16), ciphertext).tags([
            tag(["p", wallet_service.to_hex().as_str()])?,
            tag(["encryption", "nip44_v2"])?,
            tag(["expiration", expiration.to_string().as_str()])?,
        ]),
    )
}

/// Validate and decrypt a kind-23194 NWC-321 `pay` request.
pub fn decrypt_pay_request(
    event: &Event,
    wallet_service: &Keys,
) -> Result<NwcPayRequest, NwcWireError> {
    validate_recipient(event, KIND_NWC_REQUEST, &wallet_service.public_key())?;
    let plaintext = nip44::decrypt(wallet_service.secret_key(), &event.pubkey, &event.content)
        .map_err(|error| NwcWireError::Encryption(error.to_string()))?;
    let request: NwcPayRequest = serde_json::from_str(&plaintext)
        .map_err(|error| NwcWireError::Payload(error.to_string()))?;
    if request.method != "pay" || request.params.amount == 0 || request.params.payment.is_empty() {
        return Err(NwcWireError::Invalid(
            "unsupported or incomplete request".into(),
        ));
    }
    Ok(request)
}

/// Build an unsigned encrypted kind-23195 response to `request`.
pub fn build_pay_response(
    wallet_service: &Keys,
    request: &Event,
    response: &NwcPayResponse,
) -> Result<EventBuilder, NwcWireError> {
    if request.kind != Kind::Custom(KIND_NWC_REQUEST as u16) {
        return Err(NwcWireError::Invalid(
            "response target is not an NWC request".into(),
        ));
    }
    let plaintext = serde_json::to_string(response)
        .map_err(|error| NwcWireError::Payload(error.to_string()))?;
    let ciphertext = nip44::encrypt(
        wallet_service.secret_key(),
        &request.pubkey,
        plaintext,
        Version::V2,
    )
    .map_err(|error| NwcWireError::Encryption(error.to_string()))?;
    Ok(
        EventBuilder::new(Kind::Custom(KIND_NWC_RESPONSE as u16), ciphertext).tags([
            tag(["p", request.pubkey.to_hex().as_str()])?,
            tag(["e", request.id.to_hex().as_str()])?,
            tag(["encryption", "nip44_v2"])?,
        ]),
    )
}

/// Validate and decrypt a kind-23195 `pay` response.
pub fn decrypt_pay_response(event: &Event, client: &Keys) -> Result<NwcPayResponse, NwcWireError> {
    validate_recipient(event, KIND_NWC_RESPONSE, &client.public_key())?;
    let plaintext = nip44::decrypt(client.secret_key(), &event.pubkey, &event.content)
        .map_err(|error| NwcWireError::Encryption(error.to_string()))?;
    let response: NwcPayResponse = serde_json::from_str(&plaintext)
        .map_err(|error| NwcWireError::Payload(error.to_string()))?;
    if response.result_type != "pay" || response.error.is_some() == response.result.is_some() {
        return Err(NwcWireError::Invalid("malformed pay response".into()));
    }
    Ok(response)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pay_request_and_response_round_trip() {
        let client = Keys::generate();
        let wallet = Keys::generate();
        let request = build_pay_request(
            &client,
            &wallet.public_key(),
            NwcPayParams {
                payment: "bitcoin:?lno=lno1example".into(),
                amount: 21_000,
                payer_note: Some("nostr:nipB1:intent".into()),
                metadata: Default::default(),
            },
            u64::MAX,
        )
        .unwrap()
        .sign_with_keys(&client)
        .unwrap();
        let decoded = decrypt_pay_request(&request, &wallet).unwrap();
        assert_eq!(decoded.params.amount, 21_000);

        let response = build_pay_response(
            &wallet,
            &request,
            &NwcPayResponse {
                result_type: "pay".into(),
                error: None,
                result: Some(NwcPayResult {
                    transaction_id: "tx".into(),
                    state: "settled".into(),
                    instruction_type: "bolt12".into(),
                    amount: 21_000,
                    fees_paid: Some(1_000),
                    payer_proof: None,
                    failure_reason: None,
                    created_at: 1,
                    settled_at: Some(2),
                }),
            },
        )
        .unwrap()
        .sign_with_keys(&wallet)
        .unwrap();
        assert_eq!(
            decrypt_pay_response(&response, &client)
                .unwrap()
                .result
                .unwrap()
                .state,
            "settled"
        );
    }
}

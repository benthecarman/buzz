#[cfg(feature = "bitcoin")]
mod enabled {
    use buzz_core_pkg::{
        kind::{KIND_BOLT12_ZAP_INTENT, KIND_NWC_INFO},
        nwc::{
            build_pay_response, decrypt_pay_request, NwcErrorBody, NwcPayResponse, NwcPayResult,
        },
    };
    use lexe_payment_uri_core::Bip321Uri;
    use nostr::{Event, EventBuilder, JsonUtil, Kind, Tag};
    use tauri::{AppHandle, State};

    use crate::{
        app_state::AppState,
        commands::wallet::enabled::is_unsupported_wallet_event_kind,
        relay::submit_signed_event_at_with_keys,
        wallet::{
            models::{
                WalletError, WalletNwcRequest, WalletPaymentResult, WalletPaymentStatus,
            },
            zap::{recipient_offer, validate_offer_event},
        },
    };

    fn now_ms() -> u64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|duration| duration.as_millis().min(u128::from(u64::MAX)) as u64)
            .unwrap_or_default()
    }

    fn event_tag<'a>(event: &'a Event, name: &str) -> Option<&'a str> {
        event.tags.iter().find_map(|tag| {
            let parts = tag.as_slice();
            (parts.first().map(String::as_str) == Some(name))
                .then(|| parts.get(1).map(String::as_str))
                .flatten()
        })
    }

    fn event_tag_count(event: &Event, name: &str) -> usize {
        event
            .tags
            .iter()
            .filter(|tag| tag.as_slice().first().map(String::as_str) == Some(name))
            .count()
    }

    fn validate_request(
        app: &AppHandle,
        keys: &nostr::Keys,
        raw_event: &serde_json::Value,
    ) -> Result<(Event, WalletNwcRequest), WalletError> {
        let event = Event::from_json(raw_event.to_string())
            .map_err(|error| WalletError::new("invalid_nwc_request", error.to_string()))?;
        let request = decrypt_pay_request(&event, keys)
            .map_err(|error| WalletError::new("invalid_nwc_request", error.to_string()))?;
        let expiration = event_tag(&event, "expiration")
            .and_then(|value| value.parse::<u64>().ok())
            .ok_or_else(|| {
                WalletError::new("invalid_nwc_request", "request has no valid expiration")
            })?;
        if expiration < now_ms() / 1_000 {
            return Err(WalletError::new(
                "request_expired",
                "The wallet request expired",
            ));
        }
        let agent = crate::managed_agents::load_managed_agents(app)
            .map_err(WalletError::unavailable)?
            .into_iter()
            .find(|agent| agent.pubkey == event.pubkey.to_hex())
            .ok_or_else(|| {
                WalletError::new("unauthorized", "request author is not a managed agent")
            })?;
        if request.params.amount % 1_000 != 0 {
            return Err(WalletError::new(
                "invalid_amount",
                "Buzz wallets accept whole-satoshi NWC payments",
            ));
        }
        let intent_json = request
            .params
            .metadata
            .get("zap_intent")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| WalletError::new("invalid_zap", "request has no zap intent"))?;
        let intent = Event::from_json(intent_json)
            .map_err(|error| WalletError::new("invalid_zap", error.to_string()))?;
        intent
            .verify()
            .map_err(|error| WalletError::new("invalid_zap", error.to_string()))?;
        if intent.kind != Kind::Custom(KIND_BOLT12_ZAP_INTENT as u16)
            || intent.pubkey != event.pubkey
        {
            return Err(WalletError::new(
                "invalid_zap",
                "zap intent is not signed by the requesting agent",
            ));
        }
        if event_tag_count(&intent, "p") != 1
            || event_tag_count(&intent, "amount") != 1
            || event_tag_count(&intent, "offer_event") != 1
            || event_tag_count(&intent, "zap_id") != 1
            || event_tag_count(&intent, "e") > 1
            || event_tag_count(&intent, "a") > 1
            || (event_tag_count(&intent, "e") != 0 && event_tag_count(&intent, "a") != 0)
        {
            return Err(WalletError::new(
                "invalid_zap",
                "zap intent does not satisfy NIP-B1 tag cardinality",
            ));
        }
        let zap_id = event_tag(&intent, "zap_id").unwrap_or_default();
        if zap_id.len() < 32
            || !zap_id.len().is_multiple_of(2)
            || !zap_id
                .chars()
                .all(|character| character.is_ascii_hexdigit() && !character.is_ascii_uppercase())
        {
            return Err(WalletError::new(
                "invalid_zap",
                "zap intent has an invalid zap_id",
            ));
        }
        let payer_note = request
            .params
            .payer_note
            .clone()
            .ok_or_else(|| WalletError::new("invalid_zap", "request has no payer note"))?;
        if payer_note != format!("nostr:nipB1:{}", intent.id.to_hex())
            || event_tag(&intent, "amount") != Some(request.params.amount.to_string().as_str())
        {
            return Err(WalletError::new(
                "invalid_zap",
                "wallet request does not match its zap intent",
            ));
        }
        let recipient_pubkey = event_tag(&intent, "p")
            .map(str::to_string)
            .ok_or_else(|| WalletError::new("invalid_zap", "zap intent has no recipient"))?;
        let payment_uri = url::Url::parse(&request.params.payment)
            .map_err(|_| WalletError::new("invalid_zap", "payment is not a BIP-321 URI"))?;
        if payment_uri.scheme() != "bitcoin"
            || payment_uri
                .query_pairs()
                .any(|(name, _)| name == "pop" || name == "req-pop" || name.starts_with("req-"))
        {
            return Err(WalletError::new(
                "invalid_zap",
                "payment contains an unsupported BIP-321 instruction",
            ));
        }
        let selected_offers = payment_uri
            .query_pairs()
            .filter(|(name, _)| name == "lno")
            .map(|(_, value)| value.into_owned())
            .collect::<Vec<_>>();
        if selected_offers.len() != 1 {
            return Err(WalletError::new(
                "invalid_zap",
                "payment must select exactly one BOLT12 offer",
            ));
        }
        let parsed_payment = Bip321Uri::parse(&request.params.payment)
            .map_err(|error| WalletError::new("invalid_zap", error.to_string()))?;
        let selected_offer = parsed_payment
            .offer
            .ok_or_else(|| WalletError::new("invalid_zap", "payment has no valid BOLT12 offer"))?;
        let selected_offer = selected_offer.to_string();
        if selected_offer != selected_offers[0] {
            return Err(WalletError::new(
                "invalid_zap",
                "payment must contain a canonical BOLT12 offer",
            ));
        }
        let offer_event_json = event_tag(&intent, "offer_event")
            .ok_or_else(|| WalletError::new("invalid_zap", "zap intent has no offer event"))?;
        let offer_event = Event::from_json(offer_event_json)
            .map_err(|error| WalletError::new("invalid_zap", error.to_string()))?;
        validate_offer_event(&offer_event, &recipient_pubkey)?;
        recipient_offer(&offer_event, &recipient_pubkey)?;
        if offer_event.created_at > intent.created_at {
            return Err(WalletError::new(
                "invalid_zap",
                "offer announcement is newer than the zap intent",
            ));
        }
        if !offer_event.tags.iter().any(|tag| {
            let parts = tag.as_slice();
            parts.first().map(String::as_str) == Some("offer")
                && parts.get(1).map(String::as_str) == Some(selected_offer.as_str())
        }) {
            return Err(WalletError::new(
                "invalid_zap",
                "payment offer is not authorized by the recipient announcement",
            ));
        }
        let request_id = uuid::Uuid::new_v5(&uuid::Uuid::NAMESPACE_OID, event.id.as_bytes());
        let parsed = WalletNwcRequest {
            event_id: event.id.to_hex(),
            agent_pubkey: agent.pubkey,
            agent_name: agent.name,
            recipient_pubkey,
            amount: request.params.amount / 1_000,
            comment: intent.content,
            destination: request.params.payment,
            payer_note,
            request_id: request_id.to_string(),
        };
        Ok((event, parsed))
    }

    pub(crate) async fn publish_nwc_info(
        state: &AppState,
        keys: &nostr::Keys,
        relay_api_base_urls: &[String],
        enabled: bool,
    ) -> Result<Vec<String>, WalletError> {
        let mut tags = vec![Tag::parse(["encryption", "nip44_v2"])
            .map_err(|error| WalletError::new("relay_publish_failed", error.to_string()))?];
        if enabled {
            tags.push(
                Tag::parse(["extensions", "321"])
                    .map_err(|error| WalletError::new("relay_publish_failed", error.to_string()))?,
            );
        }
        let event = EventBuilder::new(
            Kind::Custom(KIND_NWC_INFO as u16),
            if enabled { "pay" } else { "" },
        )
        .tags(tags)
        .sign_with_keys(keys)
        .map_err(|error| WalletError::new("relay_publish_failed", error.to_string()))?;
        let mut warnings = Vec::new();
        for (index, relay) in relay_api_base_urls.iter().enumerate() {
            if let Err(error) = submit_signed_event_at_with_keys(&event, state, relay, keys).await {
                if index == 0 && !is_unsupported_wallet_event_kind(&error) {
                    return Err(WalletError::new(
                        "relay_publish_failed",
                        format!("publish NWC info to active community {relay}: {error}"),
                    ));
                }
                warnings.push(format!("{relay}: {error}"));
            }
        }
        Ok(warnings)
    }

    /// Validate and decrypt an agent-authored NWC-321 `pay` request.
    #[tauri::command]
    pub async fn wallet_parse_nwc_request(
        app: AppHandle,
        state: State<'_, AppState>,
        event: serde_json::Value,
    ) -> Result<WalletNwcRequest, WalletError> {
        let keys = state.signing_keys().map_err(WalletError::unavailable)?;
        validate_request(&app, &keys, &event).map(|(_, request)| request)
    }

    /// Build a signed, encrypted NWC response after approval or denial.
    #[tauri::command]
    pub async fn wallet_build_nwc_response(
        app: AppHandle,
        state: State<'_, AppState>,
        event: serde_json::Value,
        payment: Option<WalletPaymentResult>,
        error_code: Option<String>,
        error_message: Option<String>,
    ) -> Result<serde_json::Value, WalletError> {
        let keys = state.signing_keys().map_err(WalletError::unavailable)?;
        let (request_event, request) = validate_request(&app, &keys, &event)?;
        let response = match (payment, error_code, error_message) {
            (Some(payment), None, None) => {
                let state = match payment.status {
                    WalletPaymentStatus::Completed => "settled",
                    WalletPaymentStatus::Failed => "failed",
                    WalletPaymentStatus::Pending => "pending",
                };
                NwcPayResponse {
                    result_type: "pay".into(),
                    error: None,
                    result: Some(NwcPayResult {
                        transaction_id: payment.payment_id,
                        state: state.into(),
                        instruction_type: "bolt12".into(),
                        amount: payment
                            .amount
                            .unwrap_or(request.amount)
                            .saturating_mul(1_000),
                        fees_paid: Some(payment.fees.saturating_mul(1_000)),
                        // Temporary NIP-B1 marker until Lexe exposes the settled lnp proof.
                        payer_proof: (state == "settled").then(|| "placeholder".to_string()),
                        failure_reason: (state == "failed").then_some(payment.status_message),
                        created_at: payment.created_at_ms / 1_000,
                        settled_at: if state == "settled" {
                            Some(payment.finalized_at_ms.unwrap_or(payment.created_at_ms) / 1_000)
                        } else {
                            payment.finalized_at_ms.map(|value| value / 1_000)
                        },
                    }),
                }
            }
            (None, Some(code), Some(message)) => NwcPayResponse {
                result_type: "pay".into(),
                error: Some(NwcErrorBody { code, message }),
                result: None,
            },
            _ => {
                return Err(WalletError::new(
                    "invalid_nwc_response",
                    "provide either a payment result or an error",
                ))
            }
        };
        let signed = build_pay_response(&keys, &request_event, &response)
            .map_err(|error| WalletError::new("invalid_nwc_response", error.to_string()))?
            .sign_with_keys(&keys)
            .map_err(|error| WalletError::new("invalid_nwc_response", error.to_string()))?;
        serde_json::to_value(signed)
            .map_err(|error| WalletError::new("invalid_nwc_response", error.to_string()))
    }
}

#[cfg(feature = "bitcoin")]
pub use enabled::*;

#[cfg(not(feature = "bitcoin"))]
mod disabled {
    use serde::Serialize;

    #[derive(Serialize)]
    pub struct WalletDisabledError {
        code: &'static str,
        message: &'static str,
    }

    fn disabled() -> WalletDisabledError {
        WalletDisabledError {
            code: "wallet_unavailable",
            message: "this Buzz binary was built without the `bitcoin` feature",
        }
    }

    #[tauri::command]
    pub async fn wallet_parse_nwc_request(
        event: serde_json::Value,
    ) -> Result<serde_json::Value, WalletDisabledError> {
        let _ = event;
        Err(disabled())
    }

    #[tauri::command]
    pub async fn wallet_build_nwc_response(
        event: serde_json::Value,
        payment: Option<serde_json::Value>,
        error_code: Option<String>,
        error_message: Option<String>,
    ) -> Result<serde_json::Value, WalletDisabledError> {
        let _ = (event, payment, error_code, error_message);
        Err(disabled())
    }
}

#[cfg(not(feature = "bitcoin"))]
pub use disabled::*;

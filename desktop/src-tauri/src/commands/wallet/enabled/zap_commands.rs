use buzz_core_pkg::{agent_runtime_payment::RuntimePricing, kind::KIND_AGENT_RUNTIME_PRICING};
use nostr::{Event, JsonUtil, Kind};
use tauri::{AppHandle, State};

use super::{
    app_data_dir, paying_attempt_expired, payment_error, profile_payment_result,
    resolve_recipient_offer, wallet_manager, zap_target_channel_id,
};
use crate::{
    app_state::AppState,
    relay::{relay_api_base_url_with_override, submit_signed_event_at_with_keys},
    wallet::{
        models::{
            WalletAgentRuntimeZapRequest, WalletError, WalletOfferSendRequest,
            WalletProfileZapDraft, WalletProfileZapRequest, WalletProfileZapResult,
            WalletRecipientOffer,
        },
        provider::WalletPaymentMatch,
        zap::{ZapAttempt, ZapAttemptState, ZapAttemptStore},
    },
};

pub(crate) async fn reconcile_wallet_background_once(
    app: &AppHandle,
    state: &AppState,
) -> Result<bool, WalletError> {
    let reconciled_zaps = reconcile_paying_zap_attempts(app, state).await?;
    let published_proofs = reconcile_pending_zap_proofs(app, state).await?;
    Ok(reconciled_zaps || published_proofs)
}

async fn reconcile_paying_zap_attempts(
    app: &AppHandle,
    state: &AppState,
) -> Result<bool, WalletError> {
    let keys = state.signing_keys().map_err(WalletError::unavailable)?;
    let payer_pubkey = keys.public_key().to_hex();
    let data_dir = app_data_dir(app)?;
    let store = ZapAttemptStore::new(&data_dir, &payer_pubkey);
    let relay = relay_api_base_url_with_override(state);
    let candidates = store.paying_attempts_for_relay(&relay)?;
    if candidates.is_empty() {
        return Ok(false);
    }
    let provider = wallet_manager().provider_for(&keys, &data_dir).await?;
    let mut changed_any = false;

    for candidate in candidates {
        let operation_lock = wallet_manager()
            .operation_lock(&payer_pubkey, &candidate.idempotency_key)
            .await;
        let _operation_guard = operation_lock.lock().await;
        let Some(mut attempt) = store.load(&candidate.idempotency_key)? else {
            continue;
        };
        if attempt.state != ZapAttemptState::Paying
            || attempt.relay_url.as_deref().is_none_or(|attempt_relay| {
                attempt_relay.trim_end_matches('/') != relay.trim_end_matches('/')
            })
        {
            continue;
        }

        store.record_reconcile(&attempt)?;
        let personal_note = if attempt.runtime_channel_id.is_some() {
            format!("Buzz agent runtime payment {}", attempt.intent_event_id)
        } else if attempt.target_event_id.is_some() {
            format!("Buzz message zap {}", attempt.intent_event_id)
        } else {
            format!("Buzz profile payment {}", attempt.intent_event_id)
        };
        let payment_match = WalletPaymentMatch {
            payer_note: Some(&attempt.payer_note),
            personal_note: Some(&personal_note),
            expected_amount: Some(attempt.amount),
            expected_offer: Some(&attempt.offer),
        };
        let payment = match provider.find_outbound_payment(payment_match).await {
            Ok(Some(payment)) => payment,
            Ok(None) if paying_attempt_expired(attempt.updated_at_ms) => {
                store.fail_reconciliation(&mut attempt)?;
                changed_any = true;
                tracing::warn!(
                    intent_event_id = %attempt.intent_event_id,
                    "expired unresolved zap payment without sending again"
                );
                continue;
            }
            Ok(None) => continue,
            Err(error) => {
                tracing::warn!(
                    code = error.code,
                    error = %error.message,
                    intent_event_id = %attempt.intent_event_id,
                    "background zap payment lookup failed"
                );
                continue;
            }
        };

        if payment.status == "pending" {
            if attempt.payment.as_ref() != Some(&payment) {
                store.record_payment(&mut attempt, payment)?;
                changed_any = true;
            }
            continue;
        }

        let runtime_channel = attempt.runtime_channel_id.clone();
        let result = profile_payment_result(
            state,
            &keys,
            &store,
            &mut attempt,
            payment,
            runtime_channel.as_deref(),
        )
        .await;
        changed_any = true;
        if let Err(error) = result {
            tracing::warn!(
                code = error.code,
                error = %error.message,
                intent_event_id = %attempt.intent_event_id,
                "background zap payment reconciliation did not finish"
            );
        }
    }

    Ok(changed_any)
}

async fn reconcile_pending_zap_proofs(
    app: &AppHandle,
    state: &AppState,
) -> Result<bool, WalletError> {
    let keys = state.signing_keys().map_err(WalletError::unavailable)?;
    let payer_pubkey = keys.public_key().to_hex();
    let data_dir = app_data_dir(app)?;
    let store = ZapAttemptStore::new(&data_dir, &payer_pubkey);
    let relay = relay_api_base_url_with_override(state);
    let attempts = store.unpublished_proofs_for_relay(&relay)?;
    let mut published_any = false;

    for candidate in attempts {
        let operation_lock = wallet_manager()
            .operation_lock(&payer_pubkey, &candidate.idempotency_key)
            .await;
        let _operation_guard = operation_lock.lock().await;
        let Some(mut attempt) = store.load(&candidate.idempotency_key)? else {
            continue;
        };
        if attempt.state != ZapAttemptState::PaidWithoutProof
            || attempt.proof_published
            || attempt.relay_url.as_deref().is_none_or(|attempt_relay| {
                attempt_relay.trim_end_matches('/') != relay.trim_end_matches('/')
            })
        {
            continue;
        }

        let publish_result = async {
            let channel = zap_target_channel_id(state, &keys, &attempt, &relay).await?;
            let event = store.prepare_placeholder_proof(&mut attempt, &keys, channel.as_deref())?;
            submit_signed_event_at_with_keys(&event, state, &relay, &keys)
                .await
                .map_err(|error| {
                    WalletError::new(
                        "relay_publish_failed",
                        format!("republish placeholder zap proof: {error}"),
                    )
                })?;
            store.mark_proof_published(&mut attempt)
        }
        .await;

        match publish_result {
            Ok(()) => published_any = true,
            Err(error) => tracing::warn!(
                code = error.code,
                error = %error.message,
                intent_event_id = %attempt.intent_event_id,
                "background zap proof publication failed"
            ),
        }
    }

    Ok(published_any)
}

#[tauri::command]
pub async fn wallet_get_recipient_offer(
    recipient_pubkey: String,
    state: State<'_, AppState>,
) -> Result<WalletRecipientOffer, WalletError> {
    let keys = state.signing_keys().map_err(WalletError::unavailable)?;
    let relay = relay_api_base_url_with_override(&state);
    resolve_recipient_offer(&state, &keys, &relay, &recipient_pubkey).await
}

#[tauri::command]
pub async fn wallet_get_pending_profile_zap(
    app: AppHandle,
    recipient_pubkey: String,
    target_event_id: Option<String>,
    state: State<'_, AppState>,
) -> Result<Option<WalletProfileZapDraft>, WalletError> {
    let keys = state.signing_keys().map_err(WalletError::unavailable)?;
    let relay = relay_api_base_url_with_override(&state);
    ZapAttemptStore::new(&app_data_dir(&app)?, &keys.public_key().to_hex()).pending_for_recipient(
        &recipient_pubkey,
        target_event_id.as_deref(),
        &relay,
    )
}
fn validate_agent_runtime_pricing(
    pricing_event: &Event,
    agent_pubkey: &str,
) -> Result<RuntimePricing, WalletError> {
    pricing_event
        .verify()
        .map_err(|error| WalletError::new("runtime_purchase_invalid", error.to_string()))?;
    let pricing: RuntimePricing = serde_json::from_str(&pricing_event.content)
        .map_err(|error| WalletError::new("runtime_purchase_invalid", error.to_string()))?;
    pricing
        .validate()
        .map_err(|error| WalletError::new("runtime_purchase_invalid", error.to_string()))?;
    if pricing_event.pubkey.to_hex() != agent_pubkey
        || pricing_event.kind != Kind::Custom(KIND_AGENT_RUNTIME_PRICING as u16)
        || !pricing.enabled
    {
        return Err(WalletError::new(
            "runtime_purchase_invalid",
            "pricing does not match the selected agent",
        ));
    }
    Ok(pricing)
}

#[tauri::command]
pub async fn wallet_send_profile_zap(
    app: AppHandle,
    state: State<'_, AppState>,
    request: WalletProfileZapRequest,
) -> Result<WalletProfileZapResult, WalletError> {
    wallet_send_zap_attempt(app, state, request, None).await
}

#[tauri::command]
pub async fn wallet_send_agent_runtime_zap(
    app: AppHandle,
    state: State<'_, AppState>,
    request: WalletAgentRuntimeZapRequest,
) -> Result<WalletProfileZapResult, WalletError> {
    let keys = state.signing_keys().map_err(WalletError::unavailable)?;
    let pricing_event = Event::from_json(&request.pricing_event_json)
        .map_err(|error| WalletError::new("runtime_purchase_invalid", error.to_string()))?;
    let pricing = validate_agent_runtime_pricing(&pricing_event, &request.agent_pubkey)?;
    let amount_sats = pricing
        .price_sats
        .ok_or_else(|| WalletError::new("runtime_purchase_invalid", "agent pricing is disabled"))?;
    let relay = relay_api_base_url_with_override(&state);
    let recipient = resolve_recipient_offer(&state, &keys, &relay, &request.agent_pubkey).await?;
    let payment_request = WalletProfileZapRequest {
        recipient_pubkey: request.agent_pubkey,
        amount: amount_sats,
        comment: None,
        idempotency_key: request.idempotency_key,
        target_event_id: Some(pricing_event.id.to_hex()),
        target_event_kind: Some(KIND_AGENT_RUNTIME_PRICING),
    };
    wallet_send_zap_attempt(
        app,
        state,
        payment_request,
        Some((recipient, request.channel_id)),
    )
    .await
}

async fn wallet_send_zap_attempt(
    app: AppHandle,
    state: State<'_, AppState>,
    request: WalletProfileZapRequest,
    prepared_runtime: Option<(WalletRecipientOffer, String)>,
) -> Result<WalletProfileZapResult, WalletError> {
    let keys = state.signing_keys().map_err(WalletError::unavailable)?;
    let payer_pubkey = keys.public_key().to_hex();
    if payer_pubkey == request.recipient_pubkey {
        return Err(WalletError::new(
            "invalid_recipient",
            "You cannot send Bitcoin to your own profile",
        ));
    }
    let data_dir = app_data_dir(&app)?;
    let lock = wallet_manager()
        .operation_lock(&payer_pubkey, &request.idempotency_key)
        .await;
    let _guard = lock.lock().await;
    let provider = wallet_manager().provider_for(&keys, &data_dir).await?;
    let store = ZapAttemptStore::new(&data_dir, &payer_pubkey);
    store.prune()?;
    let active_relay = relay_api_base_url_with_override(&state);
    let comment = request
        .comment
        .as_deref()
        .map(str::trim)
        .filter(|comment| !comment.is_empty())
        .map(str::to_string);
    let runtime_channel = prepared_runtime
        .as_ref()
        .map(|(_, channel)| channel.clone());
    let mut attempt = match store.load(&request.idempotency_key)? {
        Some(attempt)
            if attempt.recipient_pubkey == request.recipient_pubkey
                && attempt.amount == request.amount
                && attempt.comment == comment
                && attempt.target_event_id == request.target_event_id
                && attempt.target_event_kind == request.target_event_kind
                && attempt.runtime_channel_id == runtime_channel =>
        {
            attempt
        }
        Some(attempt) => {
            store.record_conflict(&attempt);
            return Err(WalletError::new(
                "idempotency_conflict",
                "This payment key was already used for different details",
            ));
        }
        None => {
            let mut attempt = if let Some((recipient, channel_id)) = prepared_runtime {
                let pricing_event_id = request.target_event_id.clone().ok_or_else(|| {
                    WalletError::new(
                        "runtime_purchase_invalid",
                        "runtime zap has no pricing target",
                    )
                })?;
                ZapAttempt::prepare_agent_runtime(
                    request.idempotency_key,
                    recipient,
                    request.amount,
                    channel_id,
                    pricing_event_id,
                    &keys,
                )?
            } else {
                let recipient = resolve_recipient_offer(
                    &state,
                    &keys,
                    &active_relay,
                    &request.recipient_pubkey,
                )
                .await?;
                ZapAttempt::prepare(
                    request.idempotency_key,
                    recipient,
                    request.amount,
                    comment,
                    request.target_event_id,
                    request.target_event_kind,
                    &keys,
                )?
            };
            attempt.relay_url = Some(active_relay.clone());
            store.save_prepared(&mut attempt)?;
            attempt
        }
    };
    store.bind_relay_if_missing(&mut attempt, &active_relay)?;

    match attempt.state {
        ZapAttemptState::PaidWithoutProof => {
            store.record_terminal_reuse(&attempt);
            if !attempt.proof_published {
                let channel = match runtime_channel.clone() {
                    Some(channel) => Some(channel),
                    None => {
                        let relay = attempt.relay_url.as_deref().unwrap_or(&active_relay);
                        zap_target_channel_id(&state, &keys, &attempt, relay).await?
                    }
                };
                let event =
                    store.prepare_placeholder_proof(&mut attempt, &keys, channel.as_deref())?;
                let relay = attempt.relay_url.as_deref().unwrap_or(&active_relay);
                submit_signed_event_at_with_keys(&event, &state, relay, &keys)
                    .await
                    .map_err(|error| {
                        WalletError::new(
                            "relay_publish_failed",
                            format!("publish placeholder zap proof: {error}"),
                        )
                    })?;
                store.mark_proof_published(&mut attempt)?;
            }
            return attempt.result().ok_or_else(|| {
                WalletError::unavailable("settled profile payment is missing its result")
            });
        }
        ZapAttemptState::Failed => {
            store.record_terminal_reuse(&attempt);
            return Err(attempt.payment.as_ref().map_or_else(
                || WalletError::new("payment_failed", "The prior payment failed"),
                payment_error,
            ));
        }
        ZapAttemptState::Prepared | ZapAttemptState::Paying => {}
    }

    let personal_note = if attempt.runtime_channel_id.is_some() {
        format!("Buzz agent runtime payment {}", attempt.intent_event_id)
    } else if attempt.target_event_id.is_some() {
        format!("Buzz message zap {}", attempt.intent_event_id)
    } else {
        format!("Buzz profile payment {}", attempt.intent_event_id)
    };
    let payer_note = attempt.payer_note.clone();
    let expected_amount = attempt.amount;
    let expected_offer = attempt.offer.clone();
    let payment_match = || WalletPaymentMatch {
        payer_note: Some(&payer_note),
        personal_note: Some(&personal_note),
        expected_amount: Some(expected_amount),
        expected_offer: Some(&expected_offer),
    };
    let payment = match attempt.state {
        ZapAttemptState::Prepared => {
            store.begin_dispatch(&mut attempt)?;
            match provider
                .send_offer(WalletOfferSendRequest {
                    offer: attempt.offer.clone(),
                    amount: attempt.amount,
                    payer_note: attempt.payer_note.clone(),
                    personal_note: personal_note.clone(),
                })
                .await
            {
                Ok(payment) => payment,
                Err(error) => {
                    store.record_reconcile(&attempt)?;
                    provider
                        .find_outbound_payment(payment_match())
                        .await?
                        .ok_or_else(|| {
                            WalletError::new(
                                "payment_status_unknown",
                                format!("{error}. Buzz retained this attempt and will only reconcile it."),
                            )
                        })?
                }
            }
        }
        ZapAttemptState::Paying => {
            store.record_reconcile(&attempt)?;
            match provider.find_outbound_payment(payment_match()).await? {
                Some(payment) => payment,
                None if paying_attempt_expired(attempt.updated_at_ms) => {
                    store.fail_reconciliation(&mut attempt)?;
                    return Err(WalletError::new(
                        "payment_failed",
                        "No matching payment was found at the provider within 24 hours of the send; the attempt was marked failed and Buzz did not send again",
                    ));
                }
                None => {
                    return Err(WalletError::new(
                        "payment_status_unknown",
                        "The prior payment result is still unknown; Buzz did not send again",
                    ));
                }
            }
        }
        ZapAttemptState::PaidWithoutProof | ZapAttemptState::Failed => {
            return Err(WalletError::unavailable(
                "profile payment entered an invalid terminal transition",
            ));
        }
    };
    profile_payment_result(
        &state,
        &keys,
        &store,
        &mut attempt,
        payment,
        runtime_channel.as_deref(),
    )
    .await
}

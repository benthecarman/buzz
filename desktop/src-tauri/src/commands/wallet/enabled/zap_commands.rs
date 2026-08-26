use tauri::{AppHandle, State};

use super::{
    app_data_dir, paying_attempt_expired, payment_error, profile_payment_result,
    resolve_recipient_offer, wallet_manager, zap_target_channel_id,
};
use crate::{
    app_state::AppState,
    commands::wallet_nwc::reconcile_nwc_budget,
    relay::{
        relay_api_base_url_with_override, submit_signed_event_at_with_keys,
        submit_signed_event_at_with_keys_classified,
    },
    wallet::{
        models::{
            WalletError, WalletOfferSendRequest, WalletPaymentStatus, WalletProfileZapDraft,
            WalletProfileZapRequest, WalletProfileZapResult, WalletRecipientOffer,
        },
        provider::WalletPaymentMatch,
        send::{SendAttemptState, SendAttemptStore},
        zap::{ZapAttempt, ZapAttemptState, ZapAttemptStore, ZapTarget},
    },
};

fn zap_payment_personal_note(
    channel_id: Option<&str>,
    target_event_id: Option<&str>,
    intent_event_id: &str,
) -> String {
    if channel_id.is_some() {
        format!("Buzz hosted agent payment {intent_event_id}")
    } else if target_event_id.is_some() {
        format!("Buzz message zap {intent_event_id}")
    } else {
        format!("Buzz profile payment {intent_event_id}")
    }
}

fn reconciliation_requires_provider<T>(candidates: &[T]) -> bool {
    !candidates.is_empty()
}

pub(crate) async fn reconcile_wallet_background_once(
    app: &AppHandle,
    state: &AppState,
) -> Result<bool, WalletError> {
    let reconciled_sends = reconcile_paying_send_attempts(app, state).await?;
    let reconciled_zaps = reconcile_paying_zap_attempts(app, state).await?;
    let published_proofs = reconcile_pending_zap_proofs(app, state).await?;
    Ok(reconciled_sends || reconciled_zaps || published_proofs)
}

async fn reconcile_paying_send_attempts(
    app: &AppHandle,
    state: &AppState,
) -> Result<bool, WalletError> {
    let keys = state.signing_keys().map_err(WalletError::unavailable)?;
    let payer_pubkey = keys.public_key().to_hex();
    let data_dir = app_data_dir(app)?;
    let store = SendAttemptStore::new(&data_dir, &payer_pubkey);
    let candidates = store.paying_attempts()?;
    store.prune()?;
    if !reconciliation_requires_provider(&candidates) {
        return Ok(false);
    }
    let provider = wallet_manager().provider_for(&keys, &data_dir).await?;
    let mut changed_any = false;

    for candidate in candidates {
        let request_id = candidate.request.request_id.clone();
        let operation_lock = wallet_manager()
            .operation_lock(&payer_pubkey, &request_id)
            .await;
        let _operation_guard = operation_lock.lock().await;
        let Some(mut attempt) = store.load(&request_id)? else {
            continue;
        };
        if attempt.state != SendAttemptState::Paying {
            continue;
        }
        store.record_reconcile(&attempt)?;
        let analysis = match provider.analyze(attempt.request.destination.clone()).await {
            Ok(analysis) => analysis,
            Err(error) => {
                tracing::warn!(code = error.code, error = %error.message, %request_id, "background payment analysis failed");
                continue;
            }
        };
        let personal_note = format!("Buzz payment {request_id}");
        let expected_amount = attempt.request.amount.or(analysis.amount);
        let expected_invoice = (analysis.instruction_type == "bolt11")
            .then_some(analysis.normalized_destination.as_str());
        let expected_offer = (analysis.instruction_type == "bolt12")
            .then_some(analysis.normalized_destination.as_str());
        let payment_match = WalletPaymentMatch {
            payer_note: None,
            personal_note: Some(&personal_note),
            expected_amount,
            expected_offer,
            expected_invoice,
        };
        let payment = match provider.find_outbound_payment(payment_match).await {
            Ok(Some(payment)) => payment,
            Ok(None) if paying_attempt_expired(attempt.updated_at_ms) => {
                store.fail_reconciliation(&mut attempt)?;
                reconcile_nwc_budget(app, state, &request_id, false)?;
                changed_any = true;
                continue;
            }
            Ok(None) => continue,
            Err(error) => {
                tracing::warn!(code = error.code, error = %error.message, %request_id, "background payment lookup failed");
                continue;
            }
        };
        if attempt.payment.as_ref() != Some(&payment) {
            store.record_payment(&mut attempt, payment.clone())?;
            changed_any = true;
        }
        match payment.status {
            WalletPaymentStatus::Completed => reconcile_nwc_budget(app, state, &request_id, true)?,
            WalletPaymentStatus::Failed => reconcile_nwc_budget(app, state, &request_id, false)?,
            WalletPaymentStatus::Pending => {}
        }
    }

    Ok(changed_any)
}

async fn reconcile_paying_zap_attempts(
    app: &AppHandle,
    state: &AppState,
) -> Result<bool, WalletError> {
    let keys = state.signing_keys().map_err(WalletError::unavailable)?;
    let payer_pubkey = keys.public_key().to_hex();
    let data_dir = app_data_dir(app)?;
    let relay = relay_api_base_url_with_override(state);
    let store = ZapAttemptStore::new(&data_dir, &payer_pubkey);
    let candidates = store.paying_attempts_for_relay(&relay)?;
    store.prune()?;
    if !reconciliation_requires_provider(&candidates) {
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
        let personal_note = zap_payment_personal_note(
            attempt.channel_id.as_deref(),
            attempt.target_event_id.as_deref(),
            &attempt.intent_event_id,
        );
        let payment_match = WalletPaymentMatch {
            payer_note: Some(&attempt.payer_note),
            personal_note: Some(&personal_note),
            expected_amount: Some(attempt.amount),
            expected_offer: Some(&attempt.offer),
            expected_invoice: None,
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

        if payment.status == WalletPaymentStatus::Pending {
            if attempt.payment.as_ref() != Some(&payment) {
                store.record_payment(&mut attempt, payment)?;
                changed_any = true;
            }
            continue;
        }

        let target_channel = attempt.channel_id.clone();
        let result = profile_payment_result(
            state,
            &keys,
            &store,
            &mut attempt,
            payment,
            target_channel.as_deref(),
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
    let relay = relay_api_base_url_with_override(state);
    let mut published_any = false;

    for store in [ZapAttemptStore::new(&data_dir, &payer_pubkey)] {
        store.prune()?;
        for candidate in store.unpublished_proofs_for_relay(&relay)? {
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
                let channel = match attempt.channel_id.clone() {
                    Some(channel) => Some(channel),
                    None => zap_target_channel_id(state, &keys, &attempt, &relay).await?,
                };
                let event = store.prepare_proof(&mut attempt, &keys, channel.as_deref())?;
                submit_signed_event_at_with_keys_classified(&event, state, &relay, &keys)
                    .await
                    .map_err(|error| {
                        WalletError::new(
                            if error.is_retryable() {
                                "relay_publish_failed"
                            } else {
                                "relay_publish_permanent"
                            },
                            format!("republish zap proof: {error}"),
                        )
                    })?;
                store.mark_proof_published(&mut attempt)
            }
            .await;

            match publish_result {
                Ok(()) => published_any = true,
                Err(error) => {
                    tracing::warn!(
                        code = error.code,
                        error = %error.message,
                        intent_event_id = %attempt.intent_event_id,
                        retryable = true,
                        "background zap proof publication failed"
                    );
                }
            }
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
#[tauri::command]
pub async fn wallet_send_profile_zap(
    app: AppHandle,
    state: State<'_, AppState>,
    request: WalletProfileZapRequest,
) -> Result<WalletProfileZapResult, WalletError> {
    wallet_send_zap_attempt(app, state, request).await
}

async fn wallet_send_zap_attempt(
    app: AppHandle,
    state: State<'_, AppState>,
    request: WalletProfileZapRequest,
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
    let mut attempt = match store.load(&request.idempotency_key)? {
        Some(attempt)
            if attempt.recipient_pubkey == request.recipient_pubkey
                && attempt.amount == request.amount
                && attempt.comment == comment
                && attempt.target_event_id == request.target_event_id
                && attempt.target_event_kind == request.target_event_kind
                && attempt.channel_id == request.channel_id
                && attempt.lease_id == request.lease_id =>
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
            let recipient =
                resolve_recipient_offer(&state, &keys, &active_relay, &request.recipient_pubkey)
                    .await?;
            let mut attempt = ZapAttempt::prepare(
                request.idempotency_key,
                recipient,
                request.amount,
                comment,
                ZapTarget {
                    event_id: request.target_event_id,
                    event_kind: request.target_event_kind,
                    channel_id: request.channel_id,
                    lease_id: request.lease_id,
                },
                &keys,
            )?;
            attempt.relay_url = Some(active_relay.clone());
            store.save_prepared(&mut attempt)?;
            attempt
        }
    };
    store.bind_relay_if_missing(&mut attempt, &active_relay)?;

    resume_zap_attempt(&state, &keys, &store, &mut attempt, provider, &active_relay).await
}

async fn resume_zap_attempt(
    state: &AppState,
    keys: &nostr::Keys,
    store: &ZapAttemptStore,
    attempt: &mut ZapAttempt,
    provider: std::sync::Arc<dyn crate::wallet::provider::WalletProvider>,
    active_relay: &str,
) -> Result<WalletProfileZapResult, WalletError> {
    let target_channel = attempt.channel_id.clone();

    match attempt.state {
        ZapAttemptState::PaidWithoutProof => {
            store.record_terminal_reuse(attempt);
            if !attempt.proof_published {
                let channel = match target_channel.clone() {
                    Some(channel) => Some(channel),
                    None => {
                        let relay = attempt.relay_url.as_deref().unwrap_or(active_relay);
                        zap_target_channel_id(state, keys, attempt, relay).await?
                    }
                };
                let event = store.prepare_proof(attempt, keys, channel.as_deref())?;
                let relay = attempt.relay_url.as_deref().unwrap_or(active_relay);
                submit_signed_event_at_with_keys(&event, state, relay, keys)
                    .await
                    .map_err(|error| {
                        WalletError::new(
                            "relay_publish_failed",
                            format!("publish zap proof: {error}"),
                        )
                    })?;
                store.mark_proof_published(attempt)?;
            }
            return attempt.result().ok_or_else(|| {
                WalletError::unavailable("settled profile payment is missing its result")
            });
        }
        ZapAttemptState::Failed => {
            store.record_terminal_reuse(attempt);
            return Err(attempt.payment.as_ref().map_or_else(
                || WalletError::new("payment_failed", "The prior payment failed"),
                payment_error,
            ));
        }
        ZapAttemptState::Prepared | ZapAttemptState::Paying => {}
    }

    let personal_note = zap_payment_personal_note(
        attempt.channel_id.as_deref(),
        attempt.target_event_id.as_deref(),
        &attempt.intent_event_id,
    );
    let payer_note = attempt.payer_note.clone();
    let expected_amount = attempt.amount;
    let expected_offer = attempt.offer.clone();
    let payment_match = || WalletPaymentMatch {
        payer_note: Some(&payer_note),
        personal_note: Some(&personal_note),
        expected_amount: Some(expected_amount),
        expected_offer: Some(&expected_offer),
        expected_invoice: None,
    };
    let payment = match attempt.state {
        ZapAttemptState::Prepared => {
            store.begin_dispatch(attempt)?;
            match provider
                .send_offer(WalletOfferSendRequest {
                    offer: attempt.offer.clone(),
                    amount: attempt.amount,
                    payer_note: Some(attempt.payer_note.clone()),
                    personal_note: personal_note.clone(),
                    idempotency_key: attempt.idempotency_key.clone(),
                })
                .await
            {
                Ok(payment) => payment,
                Err(error) => {
                    store.record_reconcile(attempt)?;
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
            store.record_reconcile(attempt)?;
            match provider.find_outbound_payment(payment_match()).await? {
                Some(payment) => payment,
                None if paying_attempt_expired(attempt.updated_at_ms) => {
                    store.fail_reconciliation(attempt)?;
                    return Err(WalletError::new(
                        "payment_failed",
                        "No matching payment was found at the provider within 5 minutes of the send; the attempt was marked failed and Buzz did not send again",
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
        state,
        keys,
        store,
        attempt,
        payment,
        target_channel.as_deref(),
    )
    .await
}

#[cfg(test)]
mod tests {
    use super::{reconciliation_requires_provider, zap_payment_personal_note};

    #[test]
    fn hosted_agent_note_wins_when_plan_is_also_a_target() {
        assert_eq!(
            zap_payment_personal_note(Some("channel"), Some("plan"), "intent"),
            "Buzz hosted agent payment intent"
        );
    }

    #[test]
    fn reconciliation_opens_the_provider_only_for_pending_payments() {
        assert!(!reconciliation_requires_provider::<u8>(&[]));
        assert!(reconciliation_requires_provider(&[1]));
    }
}

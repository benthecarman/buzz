/// Whether this desktop binary was built with the experimental Bitcoin wallet.
#[tauri::command]
pub fn bitcoin_compile_enabled() -> bool {
    cfg!(feature = "bitcoin")
}

#[cfg(feature = "bitcoin")]
mod enabled {
    use std::{collections::HashSet, sync::OnceLock};

    use buzz_core_pkg::kind::KIND_BOLT12_OFFER;
    use futures_util::future::join_all;
    use nostr::Event;
    use tauri::{AppHandle, Manager, State};

    use crate::{
        app_state::AppState,
        relay::{
            query_relay_at_with_keys, relay_api_base_url_with_override, relay_http_base_url,
            submit_signed_event_at_with_keys,
        },
        wallet::{
            models::{
                WalletDestinationAnalysis, WalletEnableResult, WalletError, WalletFundingRequest,
                WalletOfferPublicationResult, WalletOfferSendRequest, WalletPaymentResult,
                WalletProfileZapDraft, WalletProfileZapRequest, WalletProfileZapResult,
                WalletRecipientOffer, WalletSendRequest, WalletStatus, WalletTransactionPage,
            },
            provider::WalletPaymentMatch,
            send::{SendAttempt, SendAttemptState, SendAttemptStore},
            zap::{
                build_offer_announcement, build_offer_withdrawal, recipient_offer,
                validate_offer_event, ZapAttempt, ZapAttemptState, ZapAttemptStore,
            },
            WalletManager,
        },
    };

    /// How long a `Paying` attempt is reconciled against the provider before
    /// it is declared failed. Generous on purpose: Lightning HTLC expiry
    /// resolves in-flight payments within hours, so a payment the provider
    /// still cannot see a full day after dispatch never left the wallet.
    /// Failing the attempt un-bricks the UI; it never resends.
    const PAYING_ATTEMPT_GRACE_MS: u64 = 24 * 60 * 60 * 1_000;

    fn now_ms() -> u64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|duration| duration.as_millis().min(u128::from(u64::MAX)) as u64)
            .unwrap_or_default()
    }

    fn paying_attempt_expired(updated_at_ms: u64) -> bool {
        now_ms().saturating_sub(updated_at_ms) >= PAYING_ATTEMPT_GRACE_MS
    }

    fn wallet_manager() -> &'static WalletManager {
        static MANAGER: OnceLock<WalletManager> = OnceLock::new();
        MANAGER.get_or_init(WalletManager::default)
    }

    fn app_data_dir(app: &AppHandle) -> Result<std::path::PathBuf, WalletError> {
        app.path()
            .app_data_dir()
            .map_err(|error| WalletError::unavailable(format!("resolve app data path: {error}")))
    }

    fn is_unsupported_wallet_event_kind(error: &str) -> bool {
        error.to_ascii_lowercase().contains("unknown event kind")
    }

    /// Resolve the active relay plus every configured community relay to
    /// deduplicated HTTP API bases. The active relay is always first.
    fn wallet_relay_api_base_urls(
        active_relay_api_base_url: &str,
        relay_urls: Option<Vec<String>>,
    ) -> Vec<String> {
        let mut seen = HashSet::new();
        std::iter::once(active_relay_api_base_url.to_string())
            .chain(relay_urls.unwrap_or_default())
            .map(|relay_url| relay_http_base_url(&relay_url))
            .filter(|relay_url| !relay_url.is_empty())
            .filter(|relay_url| seen.insert(relay_url.clone()))
            .collect()
    }

    /// Publish our own wallet event to the active workspace relay and every
    /// configured community relay: our own offer announcement must reach
    /// every community we joined. An "unknown event kind" rejection only
    /// warns: that community does not support wallet events yet. A failure
    /// on the active relay fails the command; failures on the other
    /// community relays only warn.
    async fn publish_wallet_event(
        state: &AppState,
        keys: &nostr::Keys,
        relay_api_base_urls: &[String],
        event: &Event,
    ) -> Result<Vec<String>, WalletError> {
        let Some(active_relay) = relay_api_base_urls.first() else {
            return Err(WalletError::new(
                "relay_publish_failed",
                "no community relay is configured",
            ));
        };

        let mut warnings = Vec::new();
        match submit_signed_event_at_with_keys(event, state, active_relay, keys).await {
            Ok(_) => {}
            Err(error) if is_unsupported_wallet_event_kind(&error) => {
                tracing::warn!(
                    relay = active_relay,
                    error,
                    event_id = %event.id,
                    "active community does not support wallet events"
                );
                warnings.push(format!("{active_relay}: {error}"));
            }
            Err(error) => {
                return Err(WalletError::new(
                    "relay_publish_failed",
                    format!("publish wallet event to active community {active_relay}: {error}"),
                ))
            }
        }

        let results = join_all(relay_api_base_urls.iter().skip(1).map(|relay| async move {
            let first = submit_signed_event_at_with_keys(event, state, relay, keys).await;
            let result = match first {
                Ok(response) => Ok(response),
                Err(_) => submit_signed_event_at_with_keys(event, state, relay, keys).await,
            };
            (relay, result)
        }))
        .await;
        warnings.extend(results.into_iter().filter_map(|(relay, result)| {
            result.err().map(|error| {
                tracing::warn!(
                    relay,
                    error,
                    event_id = %event.id,
                    "community relay rejected wallet event"
                );
                format!("{relay}: {error}")
            })
        }));
        Ok(warnings)
    }

    async fn publish_offer(
        state: &AppState,
        keys: &nostr::Keys,
        relay_api_base_urls: &[String],
        offer: &str,
    ) -> Result<Vec<String>, WalletError> {
        let event = build_offer_announcement(offer)?
            .sign_with_keys(keys)
            .map_err(|error| {
                WalletError::new(
                    "relay_publish_failed",
                    format!("sign BOLT12 offer announcement: {error}"),
                )
            })?;
        publish_wallet_event(state, keys, relay_api_base_urls, &event).await
    }

    async fn resolve_recipient_offer(
        state: &AppState,
        keys: &nostr::Keys,
        active_relay: &str,
        recipient_pubkey: &str,
    ) -> Result<WalletRecipientOffer, WalletError> {
        let events = query_relay_at_with_keys(
            state,
            active_relay,
            &[serde_json::json!({
                "kinds": [KIND_BOLT12_OFFER],
                "authors": [recipient_pubkey],
                "limit": 1
            })],
            keys,
            None,
        )
        .await
        .map_err(|error| {
            WalletError::unavailable(format!(
                "query recipient offer from active community {active_relay}: {error}"
            ))
        })?;
        // Kind 10058 is replaceable: the newest announcement per author is
        // authoritative, including an empty one that withdraws the offer. A
        // relay that missed the withdrawal must not resurrect the old offer.
        if let Some(latest) = events
            .into_iter()
            .filter(|event| validate_offer_event(event, recipient_pubkey).is_ok())
            .max_by_key(|event| event.created_at)
        {
            return recipient_offer(&latest, recipient_pubkey).map_err(|_| {
                WalletError::new(
                    "offer_missing",
                    "This user has not enabled their Bitcoin wallet",
                )
            });
        }
        Err(WalletError::new(
            "offer_missing",
            "This user has not enabled their Bitcoin wallet",
        ))
    }

    fn payment_error(payment: &WalletPaymentResult) -> WalletError {
        WalletError::new("payment_failed", payment.status_message.clone())
    }

    fn profile_payment_result(
        store: &ZapAttemptStore,
        attempt: &mut ZapAttempt,
        payment: WalletPaymentResult,
    ) -> Result<WalletProfileZapResult, WalletError> {
        attempt.payment = Some(payment.clone());
        match payment.status.as_str() {
            "completed" => {
                attempt.state = ZapAttemptState::PaidWithoutProof;
                store.save(attempt)?;
                attempt
                    .result()
                    .ok_or_else(|| WalletError::unavailable("profile payment result is incomplete"))
            }
            "failed" => {
                attempt.state = ZapAttemptState::Failed;
                store.save(attempt)?;
                Err(payment_error(&payment))
            }
            _ => {
                attempt.state = ZapAttemptState::Paying;
                store.save(attempt)?;
                Err(WalletError::new(
                    "payment_status_unknown",
                    "The payment is still pending. Retry only to reconcile this same attempt.",
                ))
            }
        }
    }

    fn generic_payment_result(
        store: &SendAttemptStore,
        attempt: &mut SendAttempt,
        payment: WalletPaymentResult,
    ) -> Result<WalletPaymentResult, WalletError> {
        attempt.payment = Some(payment.clone());
        attempt.state = match payment.status.as_str() {
            "completed" => SendAttemptState::Completed,
            "failed" => SendAttemptState::Failed,
            _ => SendAttemptState::Paying,
        };
        store.save(attempt)?;
        Ok(payment)
    }

    #[tauri::command]
    pub async fn wallet_enable(
        app: AppHandle,
        state: State<'_, AppState>,
        relay_urls: Option<Vec<String>>,
    ) -> Result<WalletEnableResult, WalletError> {
        let keys = state.signing_keys().map_err(WalletError::unavailable)?;
        let app_data_dir = app_data_dir(&app)?;
        let provider = wallet_manager().provider_for(&keys, &app_data_dir).await?;
        provider.provision().await?;
        let status = provider.status().await?;
        let offer = provider.offer(false).await?;
        let relay_api_base_urls =
            wallet_relay_api_base_urls(&relay_api_base_url_with_override(&state), relay_urls);
        let publication_warnings =
            publish_offer(&state, &keys, &relay_api_base_urls, &offer).await?;
        Ok(WalletEnableResult {
            status,
            publication_warnings,
        })
    }

    #[tauri::command]
    pub async fn wallet_disable(
        state: State<'_, AppState>,
        relay_urls: Option<Vec<String>>,
    ) -> Result<WalletOfferPublicationResult, WalletError> {
        let keys = state.signing_keys().map_err(WalletError::unavailable)?;
        let event = build_offer_withdrawal()
            .sign_with_keys(&keys)
            .map_err(|error| {
                WalletError::new(
                    "relay_publish_failed",
                    format!("sign BOLT12 offer withdrawal: {error}"),
                )
            })?;
        let relay_api_base_urls =
            wallet_relay_api_base_urls(&relay_api_base_url_with_override(&state), relay_urls);
        let publication_warnings =
            publish_wallet_event(&state, &keys, &relay_api_base_urls, &event).await?;
        Ok(WalletOfferPublicationResult {
            offer: None,
            publication_warnings,
        })
    }

    #[tauri::command]
    pub async fn wallet_get_status(
        app: AppHandle,
        state: State<'_, AppState>,
    ) -> Result<WalletStatus, WalletError> {
        let keys = state.signing_keys().map_err(WalletError::unavailable)?;
        wallet_manager()
            .provider_for(&keys, &app_data_dir(&app)?)
            .await?
            .status()
            .await
    }

    #[tauri::command]
    pub async fn wallet_create_receive_request(
        app: AppHandle,
        state: State<'_, AppState>,
    ) -> Result<WalletFundingRequest, WalletError> {
        let keys = state.signing_keys().map_err(WalletError::unavailable)?;
        wallet_manager()
            .provider_for(&keys, &app_data_dir(&app)?)
            .await?
            .funding_request()
            .await
    }

    #[tauri::command]
    pub async fn wallet_refresh_offer(
        app: AppHandle,
        state: State<'_, AppState>,
        relay_urls: Option<Vec<String>>,
    ) -> Result<WalletOfferPublicationResult, WalletError> {
        let keys = state.signing_keys().map_err(WalletError::unavailable)?;
        let offer = wallet_manager()
            .provider_for(&keys, &app_data_dir(&app)?)
            .await?
            .offer(true)
            .await?;
        let relay_api_base_urls =
            wallet_relay_api_base_urls(&relay_api_base_url_with_override(&state), relay_urls);
        let publication_warnings =
            publish_offer(&state, &keys, &relay_api_base_urls, &offer).await?;
        Ok(WalletOfferPublicationResult {
            offer: Some(offer),
            publication_warnings,
        })
    }

    #[tauri::command]
    pub async fn wallet_analyze_destination(
        app: AppHandle,
        state: State<'_, AppState>,
        destination: String,
    ) -> Result<WalletDestinationAnalysis, WalletError> {
        let keys = state.signing_keys().map_err(WalletError::unavailable)?;
        wallet_manager()
            .provider_for(&keys, &app_data_dir(&app)?)
            .await?
            .analyze(destination)
            .await
    }

    #[tauri::command]
    pub async fn wallet_get_pending_send(
        app: AppHandle,
        state: State<'_, AppState>,
    ) -> Result<Option<WalletSendRequest>, WalletError> {
        let keys = state.signing_keys().map_err(WalletError::unavailable)?;
        SendAttemptStore::new(&app_data_dir(&app)?, &keys.public_key().to_hex()).latest_pending()
    }

    #[tauri::command]
    pub async fn wallet_send(
        app: AppHandle,
        state: State<'_, AppState>,
        request: WalletSendRequest,
    ) -> Result<WalletPaymentResult, WalletError> {
        let keys = state.signing_keys().map_err(WalletError::unavailable)?;
        let payer_pubkey = keys.public_key().to_hex();
        let app_data_dir = app_data_dir(&app)?;
        let lock = wallet_manager()
            .operation_lock(&payer_pubkey, &request.request_id)
            .await;
        let _guard = lock.lock().await;
        let store = SendAttemptStore::new(&app_data_dir, &payer_pubkey);
        store.prune()?;
        let mut attempt = match store.load(&request.request_id)? {
            Some(attempt) if attempt.request == request => attempt,
            Some(_) => {
                return Err(WalletError::new(
                    "idempotency_conflict",
                    "This payment request ID was already used for different details",
                ))
            }
            None => {
                let mut attempt = SendAttempt::prepare(request);
                store.save(&mut attempt)?;
                attempt
            }
        };
        match attempt.state {
            SendAttemptState::Completed => {
                return attempt.payment.ok_or_else(|| {
                    WalletError::unavailable("terminal send attempt is missing its payment")
                })
            }
            SendAttemptState::Failed => {
                return Err(attempt.payment.as_ref().map_or_else(
                    || WalletError::new("payment_failed", "The prior payment failed"),
                    payment_error,
                ))
            }
            SendAttemptState::Prepared | SendAttemptState::Paying => {}
        }
        let provider = wallet_manager().provider_for(&keys, &app_data_dir).await?;
        let personal_note = format!("Buzz payment {}", attempt.request.request_id);
        let expected_amount = match attempt.request.amount {
            Some(amount) => Some(amount),
            None => {
                provider
                    .analyze(attempt.request.destination.clone())
                    .await?
                    .amount
            }
        };
        let payment_match = || WalletPaymentMatch {
            payer_note: None,
            personal_note: Some(&personal_note),
            expected_amount,
            expected_offer: None,
        };
        let payment = match attempt.state {
            SendAttemptState::Prepared => {
                attempt.state = SendAttemptState::Paying;
                store.save(&mut attempt)?;
                match provider.send(attempt.request.clone()).await {
                    Ok(payment) => payment,
                    Err(error) => provider
                        .find_outbound_payment(payment_match())
                        .await?
                        .ok_or_else(|| {
                            WalletError::new(
                                "payment_status_unknown",
                                format!(
                                    "{error}. Buzz retained this request and will only reconcile it."
                                ),
                            )
                        })?,
                }
            }
            SendAttemptState::Paying => {
                match provider.find_outbound_payment(payment_match()).await? {
                    Some(payment) => payment,
                    None if paying_attempt_expired(attempt.updated_at_ms) => {
                        attempt.state = SendAttemptState::Failed;
                        store.save(&mut attempt)?;
                        return Err(WalletError::new(
                            "payment_failed",
                            "No matching payment was found at the provider within 24 hours of the \
                         send; the attempt was marked failed and Buzz did not send again",
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
            SendAttemptState::Completed | SendAttemptState::Failed => {
                return Err(WalletError::unavailable(
                    "send attempt entered an invalid terminal transition",
                ))
            }
        };
        generic_payment_result(&store, &mut attempt, payment)
    }

    #[tauri::command]
    pub async fn wallet_list_transactions(
        app: AppHandle,
        state: State<'_, AppState>,
        cursor: Option<String>,
        limit: Option<usize>,
        sync: Option<bool>,
    ) -> Result<WalletTransactionPage, WalletError> {
        let keys = state.signing_keys().map_err(WalletError::unavailable)?;
        wallet_manager()
            .provider_for(&keys, &app_data_dir(&app)?)
            .await?
            .transactions(cursor, limit.unwrap_or(25), sync.unwrap_or(true))
            .await
    }

    #[tauri::command]
    pub async fn wallet_poll_updates(
        app: AppHandle,
        state: State<'_, AppState>,
    ) -> Result<bool, WalletError> {
        let keys = state.signing_keys().map_err(WalletError::unavailable)?;
        wallet_manager()
            .provider_for(&keys, &app_data_dir(&app)?)
            .await?
            .poll_updates()
            .await
    }

    #[tauri::command]
    pub async fn wallet_get_recipient_offer(
        recipient_pubkey: String,
        state: State<'_, AppState>,
    ) -> Result<WalletRecipientOffer, WalletError> {
        let keys = state.signing_keys().map_err(WalletError::unavailable)?;
        let active_relay = relay_api_base_url_with_override(&state);
        resolve_recipient_offer(&state, &keys, &active_relay, &recipient_pubkey).await
    }

    #[tauri::command]
    pub async fn wallet_get_pending_profile_zap(
        app: AppHandle,
        recipient_pubkey: String,
        state: State<'_, AppState>,
    ) -> Result<Option<WalletProfileZapDraft>, WalletError> {
        let keys = state.signing_keys().map_err(WalletError::unavailable)?;
        ZapAttemptStore::new(&app_data_dir(&app)?, &keys.public_key().to_hex())
            .pending_for_recipient(&recipient_pubkey)
    }

    #[tauri::command]
    pub async fn wallet_send_profile_zap(
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
        let app_data_dir = app_data_dir(&app)?;
        let lock = wallet_manager()
            .operation_lock(&payer_pubkey, &request.idempotency_key)
            .await;
        let _guard = lock.lock().await;
        let provider = wallet_manager().provider_for(&keys, &app_data_dir).await?;
        let store = ZapAttemptStore::new(&app_data_dir, &payer_pubkey);
        store.prune()?;
        let normalized_comment = request
            .comment
            .as_deref()
            .map(str::trim)
            .filter(|comment| !comment.is_empty())
            .map(str::to_string);
        let mut attempt = match store.load(&request.idempotency_key)? {
            Some(attempt)
                if attempt.recipient_pubkey == request.recipient_pubkey
                    && attempt.amount == request.amount
                    && attempt.comment == normalized_comment =>
            {
                attempt
            }
            Some(_) => {
                return Err(WalletError::new(
                    "idempotency_conflict",
                    "This payment key was already used for different details",
                ))
            }
            None => {
                let active_relay = relay_api_base_url_with_override(&state);
                let recipient = resolve_recipient_offer(
                    &state,
                    &keys,
                    &active_relay,
                    &request.recipient_pubkey,
                )
                .await?;
                let mut attempt = ZapAttempt::prepare(
                    request.idempotency_key,
                    recipient,
                    request.amount,
                    normalized_comment,
                    &keys,
                )?;
                store.save(&mut attempt)?;
                attempt
            }
        };

        match attempt.state {
            ZapAttemptState::PaidWithoutProof => {
                return attempt.result().ok_or_else(|| {
                    WalletError::unavailable("settled profile payment is missing its result")
                })
            }
            ZapAttemptState::Failed => {
                return Err(attempt.payment.as_ref().map_or_else(
                    || WalletError::new("payment_failed", "The prior payment failed"),
                    payment_error,
                ))
            }
            ZapAttemptState::Prepared | ZapAttemptState::Paying => {}
        }

        let personal_note = format!("Buzz profile payment {}", attempt.intent_event_id);
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
                attempt.state = ZapAttemptState::Paying;
                store.save(&mut attempt)?;
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
                    Err(error) => provider
                        .find_outbound_payment(payment_match())
                        .await?
                        .ok_or_else(|| {
                            WalletError::new(
                                "payment_status_unknown",
                                format!(
                                    "{error}. Buzz retained this attempt and will only reconcile it."
                                ),
                            )
                        })?,
                }
            }
            ZapAttemptState::Paying => match provider.find_outbound_payment(payment_match()).await?
            {
                Some(payment) => payment,
                None if paying_attempt_expired(attempt.updated_at_ms) => {
                    attempt.state = ZapAttemptState::Failed;
                    store.save(&mut attempt)?;
                    return Err(WalletError::new(
                        "payment_failed",
                        "No matching payment was found at the provider within 24 hours of the \
                         send; the attempt was marked failed and Buzz did not send again",
                    ));
                }
                None => {
                    return Err(WalletError::new(
                        "payment_status_unknown",
                        "The prior payment result is still unknown; Buzz did not send again",
                    ));
                }
            },
            ZapAttemptState::PaidWithoutProof | ZapAttemptState::Failed => {
                return Err(WalletError::unavailable(
                    "profile payment entered an invalid terminal transition",
                ));
            }
        };
        profile_payment_result(&store, &mut attempt, payment)
    }

    #[tauri::command]
    pub fn wallet_reveal_recovery_phrase(
        state: State<'_, AppState>,
    ) -> Result<String, WalletError> {
        let keys = state.signing_keys().map_err(WalletError::unavailable)?;
        wallet_manager()
            .recovery_phrase(&keys)
            .map(|phrase| phrase.to_string())
    }

    #[cfg(test)]
    mod tests {
        use super::{
            is_unsupported_wallet_event_kind, paying_attempt_expired, PAYING_ATTEMPT_GRACE_MS,
        };

        #[test]
        fn only_unknown_kind_rejections_are_unsupported() {
            assert!(is_unsupported_wallet_event_kind(
                "relay returned 400 Bad Request: restricted: unknown event kind"
            ));
            assert!(is_unsupported_wallet_event_kind(
                "relay rejected event: UNKNOWN EVENT KIND"
            ));
            assert!(!is_unsupported_wallet_event_kind(
                "relay returned 401 Unauthorized"
            ));
            assert!(!is_unsupported_wallet_event_kind(
                "network error: connection refused"
            ));
        }

        #[test]
        fn paying_attempt_expires_only_after_the_grace_period() {
            let now = super::now_ms();
            assert!(!paying_attempt_expired(now));
            assert!(!paying_attempt_expired(
                now.saturating_sub(PAYING_ATTEMPT_GRACE_MS - 1)
            ));
            assert!(paying_attempt_expired(
                now.saturating_sub(PAYING_ATTEMPT_GRACE_MS)
            ));
        }
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

    macro_rules! disabled_async_command {
        ($name:ident ( $($argument:ident : $type:ty),* $(,)? ) -> $result:ty) => {
            #[tauri::command]
            pub async fn $name($($argument: $type),*) -> Result<$result, WalletDisabledError> {
                $(let _ = $argument;)*
                Err(disabled())
            }
        };
    }

    disabled_async_command!(wallet_enable(relay_urls: Option<Vec<String>>) -> serde_json::Value);
    disabled_async_command!(wallet_disable(relay_urls: Option<Vec<String>>) -> serde_json::Value);
    disabled_async_command!(wallet_get_status() -> serde_json::Value);
    disabled_async_command!(wallet_create_receive_request() -> serde_json::Value);
    disabled_async_command!(wallet_refresh_offer(relay_urls: Option<Vec<String>>) -> serde_json::Value);
    disabled_async_command!(wallet_analyze_destination(destination: String) -> serde_json::Value);
    disabled_async_command!(wallet_get_pending_send() -> serde_json::Value);
    disabled_async_command!(wallet_send(request: serde_json::Value) -> serde_json::Value);
    disabled_async_command!(
        wallet_list_transactions(
            cursor: Option<String>,
            limit: Option<usize>,
            sync: Option<bool>,
        ) -> serde_json::Value
    );
    disabled_async_command!(wallet_poll_updates() -> bool);
    disabled_async_command!(
        wallet_get_recipient_offer(recipient_pubkey: String) -> serde_json::Value
    );
    disabled_async_command!(
        wallet_get_pending_profile_zap(recipient_pubkey: String) -> serde_json::Value
    );
    disabled_async_command!(
        wallet_send_profile_zap(request: serde_json::Value) -> serde_json::Value
    );

    #[tauri::command]
    pub fn wallet_reveal_recovery_phrase() -> Result<String, WalletDisabledError> {
        Err(disabled())
    }
}

#[cfg(not(feature = "bitcoin"))]
pub use disabled::*;

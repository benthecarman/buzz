/// Whether this desktop binary was built with the experimental Bitcoin wallet.
#[tauri::command]
pub fn bitcoin_compile_enabled() -> bool {
    cfg!(feature = "bitcoin")
}

#[cfg(feature = "bitcoin")]
pub(crate) mod enabled {
    mod zap_commands;
    use std::{
        collections::{HashMap, HashSet},
        sync::{atomic::Ordering, Arc, OnceLock},
    };
    pub use zap_commands::{
        wallet_get_pending_profile_zap, wallet_get_recipient_offer, wallet_send_profile_zap,
    };

    const INCOMING_PAYMENT_EVENT: &str = "wallet-incoming-payment";
    const INCOMING_PAYMENT_POLL_SECS: u64 = 5;

    #[derive(Default)]
    struct IncomingPaymentTracker {
        completed_by_wallet: HashMap<String, HashSet<String>>,
    }

    impl IncomingPaymentTracker {
        fn observe(
            &mut self,
            wallet_pubkey: &str,
            transactions: &[WalletTransaction],
        ) -> Vec<WalletTransaction> {
            let completed = transactions
                .iter()
                .filter(|transaction| {
                    transaction.direction == "inbound" && transaction.status == "completed"
                })
                .collect::<Vec<_>>();
            let Some(seen) = self.completed_by_wallet.get_mut(wallet_pubkey) else {
                self.completed_by_wallet.insert(
                    wallet_pubkey.to_string(),
                    completed
                        .into_iter()
                        .map(|transaction| transaction.id.clone())
                        .collect(),
                );
                return Vec::new();
            };

            completed
                .into_iter()
                .filter(|transaction| seen.insert(transaction.id.clone()))
                .cloned()
                .collect()
        }

        fn clear(&mut self) {
            self.completed_by_wallet.clear();
        }

        fn has_baseline(&self, wallet_pubkey: &str) -> bool {
            self.completed_by_wallet.contains_key(wallet_pubkey)
        }
    }

    fn incoming_payment_tracker() -> &'static tokio::sync::Mutex<IncomingPaymentTracker> {
        static TRACKER: OnceLock<tokio::sync::Mutex<IncomingPaymentTracker>> = OnceLock::new();
        TRACKER.get_or_init(Default::default)
    }

    async fn ensure_incoming_payment_baseline(
        wallet_pubkey: &str,
        provider: &Arc<dyn WalletProvider>,
    ) -> Result<(), WalletError> {
        if incoming_payment_tracker()
            .lock()
            .await
            .has_baseline(wallet_pubkey)
        {
            return Ok(());
        }
        provider.poll_updates().await?;
        let page = provider.transactions(None, 100, false).await?;
        // A concurrent baseline can win between the check and this write. In
        // that case observe is idempotent and its empty/new result is ignored.
        incoming_payment_tracker()
            .lock()
            .await
            .observe(wallet_pubkey, &page.transactions);
        Ok(())
    }

    async fn poll_incoming_payments_once(
        app: &AppHandle,
        state: &AppState,
    ) -> Result<(), WalletError> {
        if !state.wallet_polling_enabled.load(Ordering::Acquire) {
            return Ok(());
        }
        let keys = state.signing_keys().map_err(WalletError::unavailable)?;
        let wallet_pubkey = keys.public_key().to_hex();
        let provider = wallet_manager()
            .provider_for(&keys, &app_data_dir(app)?)
            .await?;
        provider.poll_updates().await?;
        // Always inspect the provider's current snapshot. Another caller may
        // have performed the sync first, in which case poll_updates reports no
        // change even though this listener has not observed the payment yet.
        let page = provider.transactions(None, 100, false).await?;
        // Capture every value needed by renderer consumers before marking the
        // payment as seen. A failed snapshot is retried on the next cycle.
        let status = provider.status().await?;
        let incoming = incoming_payment_tracker()
            .lock()
            .await
            .observe(&wallet_pubkey, &page.transactions);
        if !state.wallet_polling_enabled.load(Ordering::Acquire) {
            return Ok(());
        }
        for transaction in incoming {
            let event = WalletIncomingPaymentEvent {
                transaction,
                status: status.clone(),
                transactions: page.transactions.clone(),
            };
            if let Err(error) = app.emit(INCOMING_PAYMENT_EVENT, &event) {
                tracing::warn!(error = %error, "emit incoming wallet payment");
            }
        }
        Ok(())
    }

    /// Start wallet reconciliation for the lifetime of the application.
    /// Tauri owns these tasks so outgoing zap recovery and ordinary incoming
    /// payment detection do not depend on which React screen is visible.
    pub fn start_wallet_reconciler(app: AppHandle) {
        let zap_app = app.clone();
        tauri::async_runtime::spawn(async move {
            let mut consecutive_failures = 0u32;
            loop {
                let state = zap_app.state::<AppState>();
                match zap_commands::reconcile_wallet_background_once(&zap_app, &state).await {
                    Ok(_) => consecutive_failures = 0,
                    Err(error) => {
                        consecutive_failures = consecutive_failures.saturating_add(1);
                        tracing::warn!(
                            code = error.code,
                            error = %error.message,
                            "background wallet reconciliation failed"
                        );
                    }
                }
                let multiplier = 1u64 << consecutive_failures.min(4);
                tokio::time::sleep(std::time::Duration::from_secs(
                    15u64.saturating_mul(multiplier).min(5 * 60),
                ))
                .await;
            }
        });
        tauri::async_runtime::spawn(async move {
            let mut consecutive_failures = 0u32;
            loop {
                let state = app.state::<AppState>();
                let enabled = state.wallet_polling_enabled.load(Ordering::Acquire);
                if enabled {
                    match poll_incoming_payments_once(&app, &state).await {
                        Ok(()) => consecutive_failures = 0,
                        Err(error) => {
                            consecutive_failures = consecutive_failures.saturating_add(1);
                            tracing::warn!(
                                code = error.code,
                                error = %error.message,
                                "incoming wallet payment poll failed"
                            );
                        }
                    }
                } else {
                    consecutive_failures = 0;
                }
                let multiplier = 1u64 << consecutive_failures.min(4);
                tokio::time::sleep(std::time::Duration::from_secs(
                    INCOMING_PAYMENT_POLL_SECS
                        .saturating_mul(multiplier)
                        .min(60),
                ))
                .await;
            }
        });
    }

    use buzz_core_pkg::kind::KIND_BOLT12_OFFER;
    use futures_util::future::join_all;
    use nostr::Event;
    use tauri::{AppHandle, Emitter, Manager, State};

    use crate::{
        app_state::AppState,
        relay::{
            query_relay_at_with_keys, relay_api_base_url_with_override, relay_http_base_url,
            submit_signed_event_at_with_keys,
        },
        wallet::{
            models::{
                WalletDestinationAnalysis, WalletEnableResult, WalletError, WalletFundingRequest,
                WalletIncomingPaymentEvent, WalletOfferPublicationResult, WalletPaymentResult,
                WalletProfileZapResult, WalletRecipientOffer, WalletSendRequest, WalletStatus,
                WalletTransaction, WalletTransactionPage, WalletVerifiedZapEvent,
            },
            offer_conformance::OfferPublicationTrace,
            provider::{WalletPaymentMatch, WalletProvider},
            send::{SendAttempt, SendAttemptState, SendAttemptStore},
            zap::{
                build_offer_announcement, build_offer_withdrawal, parse_tagged_zap_event,
                recipient_offer, validate_offer_event, ZapAttempt, ZapAttemptStore,
            },
            WalletManager,
        },
    };

    /// How long a `Paying` attempt is reconciled against the provider before
    /// it is declared failed and the user can start a new attempt.
    const PAYING_ATTEMPT_GRACE_MS: u64 = 5 * 60 * 1_000;

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

    /// Validate untrusted relay zap events in one native call. Invalid events
    /// are omitted because they are normal noise on a public relay.
    #[tauri::command]
    pub fn wallet_parse_zap_events(
        events: Vec<serde_json::Value>,
        allowed_recipient_pubkeys: Option<Vec<String>>,
    ) -> Vec<WalletVerifiedZapEvent> {
        let allowed = allowed_recipient_pubkeys.map(|pubkeys| {
            pubkeys
                .into_iter()
                .map(|pubkey| pubkey.trim().to_ascii_lowercase())
                .collect::<HashSet<_>>()
        });
        events
            .iter()
            .filter_map(|event| match parse_tagged_zap_event(event) {
                Ok(zap) => Some(zap),
                Err(error) => {
                    tracing::warn!(
                        event_id = event
                            .get("id")
                            .and_then(serde_json::Value::as_str)
                            .unwrap_or("unknown"),
                        code = error.code,
                        error = %error.message,
                        "rejected received zap proof"
                    );
                    None
                }
            })
            .filter(|zap| {
                allowed
                    .as_ref()
                    .is_none_or(|pubkeys| pubkeys.contains(&zap.recipient_pubkey))
            })
            .collect()
    }

    fn app_data_dir(app: &AppHandle) -> Result<std::path::PathBuf, WalletError> {
        app.path()
            .app_data_dir()
            .map_err(|error| WalletError::unavailable(format!("resolve app data path: {error}")))
    }

    pub(crate) fn is_unsupported_wallet_event_kind(error: &str) -> bool {
        error.to_ascii_lowercase().contains("unknown event kind")
    }

    /// Resolve deduplicated community HTTP API bases, with the active relay first.
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

    /// Publish to each community relay. An "unknown event kind" rejection only
    /// warns: that community does not support wallet events yet. A failure
    /// on the active relay fails the command; failures on the other
    /// community relays only warn.
    async fn publish_wallet_event(
        state: &AppState,
        keys: &nostr::Keys,
        offer_issuer: Option<&nostr::Keys>,
        relay_api_base_urls: &[String],
        event: &Event,
    ) -> Result<Vec<String>, WalletError> {
        let Some(active_relay) = relay_api_base_urls.first() else {
            return Err(WalletError::new(
                "relay_publish_failed",
                "no community relay is configured",
            ));
        };
        let wallet_owner = state.signing_keys().map_err(WalletError::unavailable)?;
        let publication_trace = OfferPublicationTrace::start(
            event,
            keys,
            offer_issuer,
            &wallet_owner,
            relay_api_base_urls,
        );

        let mut warnings = Vec::new();
        match submit_signed_event_at_with_keys(event, state, active_relay, keys).await {
            Ok(_) => publication_trace.relay_result(active_relay, true),
            Err(error) if is_unsupported_wallet_event_kind(&error) => {
                publication_trace.relay_result(active_relay, false);
                tracing::warn!(
                    relay = active_relay,
                    error,
                    event_id = %event.id,
                    "active community does not support wallet events"
                );
                warnings.push(format!("{active_relay}: {error}"));
            }
            Err(error) => {
                publication_trace.relay_result(active_relay, false);
                publication_trace.abort();
                return Err(WalletError::new(
                    "relay_publish_failed",
                    format!("publish wallet event to active community {active_relay}: {error}"),
                ));
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
            publication_trace.relay_result(relay, result.is_ok());
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
        publication_trace.finish();
        Ok(warnings)
    }

    async fn publish_offer(
        state: &AppState,
        keys: &nostr::Keys,
        offer_issuer: &nostr::Keys,
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
        publish_wallet_event(state, keys, Some(offer_issuer), relay_api_base_urls, &event).await
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

    async fn zap_target_channel_id(
        state: &AppState,
        keys: &nostr::Keys,
        attempt: &ZapAttempt,
        relay: &str,
    ) -> Result<Option<String>, WalletError> {
        let (Some(target_event_id), Some(target_event_kind)) = (
            attempt.target_event_id.as_deref(),
            attempt.target_event_kind,
        ) else {
            return Ok(None);
        };
        let events = query_relay_at_with_keys(
            state,
            relay,
            &[serde_json::json!({
                "ids": [target_event_id],
                "kinds": [target_event_kind],
                "limit": 1
            })],
            keys,
            None,
        )
        .await
        .map_err(|error| {
            WalletError::unavailable(format!("resolve zap target channel: {error}"))
        })?;
        Ok(events.into_iter().find_map(|event| {
            event.tags.iter().find_map(|tag| {
                let parts = tag.as_slice();
                (parts.first().map(String::as_str) == Some("h"))
                    .then(|| parts.get(1).cloned())
                    .flatten()
            })
        }))
    }

    async fn profile_payment_result(
        state: &AppState,
        keys: &nostr::Keys,
        store: &ZapAttemptStore,
        attempt: &mut ZapAttempt,
        payment: WalletPaymentResult,
        channel_override: Option<&str>,
    ) -> Result<WalletProfileZapResult, WalletError> {
        store.record_payment(attempt, payment.clone())?;
        match payment.status.as_str() {
            "completed" => {
                let relay = attempt
                    .relay_url
                    .clone()
                    .unwrap_or_else(|| relay_api_base_url_with_override(state));
                let channel_id = match channel_override {
                    Some(channel_id) => Some(channel_id.to_string()),
                    None => zap_target_channel_id(state, keys, attempt, &relay).await?,
                };
                let event =
                    store.prepare_placeholder_proof(attempt, keys, channel_id.as_deref())?;
                submit_signed_event_at_with_keys(&event, state, &relay, keys)
                    .await
                    .map_err(|error| {
                        WalletError::new(
                            "relay_publish_failed",
                            format!("publish placeholder zap proof: {error}"),
                        )
                    })?;
                store.mark_proof_published(attempt)?;
                attempt
                    .result()
                    .ok_or_else(|| WalletError::unavailable("profile payment result is incomplete"))
            }
            "failed" => Err(payment_error(&payment)),
            _ => Err(WalletError::new(
                "payment_status_unknown",
                "The payment is still pending. Retry only to reconcile this same attempt.",
            )),
        }
    }

    fn generic_payment_result(
        store: &SendAttemptStore,
        attempt: &mut SendAttempt,
        payment: WalletPaymentResult,
    ) -> Result<WalletPaymentResult, WalletError> {
        store.record_payment(attempt, payment.clone())?;
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
        provider.signup().await?;
        let status = provider.status().await?;
        ensure_incoming_payment_baseline(&keys.public_key().to_hex(), &provider).await?;
        let offer = provider.offer(false).await?;
        let relay_api_base_urls =
            wallet_relay_api_base_urls(&relay_api_base_url_with_override(&state), relay_urls);
        let mut publication_warnings =
            publish_offer(&state, &keys, &keys, &relay_api_base_urls, &offer).await?;
        publication_warnings.extend(
            super::super::wallet_nwc::publish_nwc_info(&state, &keys, &relay_api_base_urls, true)
                .await?,
        );
        state.wallet_polling_enabled.store(true, Ordering::Release);
        Ok(WalletEnableResult {
            status,
            publication_warnings,
        })
    }

    #[tauri::command]
    pub async fn wallet_disable(
        _app: AppHandle,
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
        let mut publication_warnings =
            publish_wallet_event(&state, &keys, None, &relay_api_base_urls, &event).await?;
        publication_warnings.extend(
            super::super::wallet_nwc::publish_nwc_info(&state, &keys, &relay_api_base_urls, false)
                .await?,
        );
        state.wallet_polling_enabled.store(false, Ordering::Release);
        incoming_payment_tracker().lock().await.clear();
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
    /// Republish the wallet owner's offer and NWC information.
    ///
    /// `rotate` mints a fresh owner offer, invalidating the published one, so
    /// it is opt-in — the ordinary repair path must not silently change an
    /// offer the owner has already shared.
    pub async fn wallet_refresh_offer(
        app: AppHandle,
        state: State<'_, AppState>,
        relay_urls: Option<Vec<String>>,
        rotate: Option<bool>,
    ) -> Result<WalletOfferPublicationResult, WalletError> {
        let keys = state.signing_keys().map_err(WalletError::unavailable)?;
        let provider = wallet_manager()
            .provider_for(&keys, &app_data_dir(&app)?)
            .await?;
        let offer = provider.offer(rotate.unwrap_or(false)).await?;
        let relay_api_base_urls =
            wallet_relay_api_base_urls(&relay_api_base_url_with_override(&state), relay_urls);
        let mut publication_warnings =
            publish_offer(&state, &keys, &keys, &relay_api_base_urls, &offer).await?;
        publication_warnings.extend(
            super::super::wallet_nwc::publish_nwc_info(&state, &keys, &relay_api_base_urls, true)
                .await?,
        );
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
            Some(attempt) => {
                store.record_conflict(&attempt);
                return Err(WalletError::new(
                    "idempotency_conflict",
                    "This payment request ID was already used for different details",
                ));
            }
            None => {
                let mut attempt = SendAttempt::prepare(request);
                store.save_prepared(&mut attempt)?;
                attempt
            }
        };
        match attempt.state {
            SendAttemptState::Completed => {
                store.record_terminal_reuse(&attempt);
                return attempt.payment.ok_or_else(|| {
                    WalletError::unavailable("terminal send attempt is missing its payment")
                });
            }
            SendAttemptState::Failed => {
                store.record_terminal_reuse(&attempt);
                return Err(attempt.payment.as_ref().map_or_else(
                    || WalletError::new("payment_failed", "The prior payment failed"),
                    payment_error,
                ));
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
                store.begin_dispatch(&mut attempt)?;
                match provider.send(attempt.request.clone()).await {
                    Ok(payment) => payment,
                    Err(error) => {
                        store.record_reconcile(&attempt)?;
                        provider
                            .find_outbound_payment(payment_match())
                            .await?
                            .ok_or_else(|| {
                                WalletError::new(
                                    "payment_status_unknown",
                                    format!(
                                        "{error}. Buzz retained this request and will only reconcile it."
                                    ),
                                )
                            })?
                    }
                }
            }
            SendAttemptState::Paying => {
                store.record_reconcile(&attempt)?;
                match provider.find_outbound_payment(payment_match()).await? {
                    Some(payment) => payment,
                    None if paying_attempt_expired(attempt.updated_at_ms) => {
                        store.fail_reconciliation(&mut attempt)?;
                        return Err(WalletError::new(
                            "payment_failed",
                            "No matching payment was found at the provider within 5 minutes of the \
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

    /// Restore the frontend-owned wallet feature setting after startup.
    /// Provision an enabled wallet for current provider releases before
    /// payment polling starts. This command does not sign up or disable a
    /// wallet.
    #[tauri::command]
    pub async fn wallet_set_polling_enabled(
        app: AppHandle,
        enabled: bool,
        state: State<'_, AppState>,
    ) -> Result<(), WalletError> {
        if enabled {
            let keys = state.signing_keys().map_err(WalletError::unavailable)?;
            let provider = wallet_manager()
                .provider_for(&keys, &app_data_dir(&app)?)
                .await?;
            provider.provision().await?;
            // Baseline before activation so a payment received immediately
            // after this command is observed as new by the next cycle.
            let baseline =
                ensure_incoming_payment_baseline(&keys.public_key().to_hex(), &provider).await;
            state.wallet_polling_enabled.store(true, Ordering::Release);
            return baseline;
        }
        state.wallet_polling_enabled.store(false, Ordering::Release);
        incoming_payment_tracker().lock().await.clear();
        Ok(())
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
            is_unsupported_wallet_event_kind, paying_attempt_expired, wallet_relay_api_base_urls,
            IncomingPaymentTracker, WalletIncomingPaymentEvent, WalletStatus, WalletTransaction,
            PAYING_ATTEMPT_GRACE_MS,
        };

        fn transaction(id: &str, direction: &str, status: &str) -> WalletTransaction {
            WalletTransaction {
                id: id.to_string(),
                direction: direction.to_string(),
                status: status.to_string(),
                status_message: status.to_string(),
                amount: Some(21),
                fees: 0,
                note: None,
                payer_note: None,
                offer_id: None,
                created_at_ms: 1,
                finalized_at_ms: (status == "completed").then_some(2),
            }
        }

        #[test]
        fn wallet_relays_include_active_and_deduplicate_all_communities() {
            assert_eq!(
                wallet_relay_api_base_urls(
                    "https://active.example",
                    Some(vec![
                        "wss://other.example".to_string(),
                        "wss://active.example".to_string(),
                        "wss://other.example/".to_string(),
                    ]),
                ),
                vec![
                    "https://active.example".to_string(),
                    "https://other.example".to_string(),
                ]
            );
        }

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
            assert_eq!(PAYING_ATTEMPT_GRACE_MS, 5 * 60 * 1_000);
            let now = super::now_ms();
            assert!(!paying_attempt_expired(now));
            assert!(!paying_attempt_expired(
                now.saturating_sub(PAYING_ATTEMPT_GRACE_MS - 1)
            ));
            assert!(paying_attempt_expired(
                now.saturating_sub(PAYING_ATTEMPT_GRACE_MS)
            ));
        }

        #[test]
        fn incoming_payment_tracker_baselines_then_emits_each_completed_inbound_once() {
            let mut tracker = IncomingPaymentTracker::default();
            let existing = transaction("existing", "inbound", "completed");
            assert!(tracker
                .observe("wallet", std::slice::from_ref(&existing))
                .is_empty());

            let pending = transaction("new", "inbound", "pending");
            let outbound = transaction("outbound", "outbound", "completed");
            assert!(tracker
                .observe("wallet", &[existing.clone(), pending, outbound])
                .is_empty());

            let completed = transaction("new", "inbound", "completed");
            assert_eq!(
                tracker.observe("wallet", &[existing.clone(), completed.clone()]),
                vec![completed.clone()]
            );
            assert!(tracker.observe("wallet", &[existing, completed]).is_empty());
        }

        #[test]
        fn incoming_payment_tracker_uses_an_independent_baseline_per_wallet() {
            let mut tracker = IncomingPaymentTracker::default();
            let payment = transaction("same-provider-id", "inbound", "completed");
            assert!(tracker
                .observe("alice", std::slice::from_ref(&payment))
                .is_empty());
            assert!(tracker.observe("bob", &[payment]).is_empty());
        }

        #[test]
        fn incoming_payment_event_contains_the_authoritative_snapshot() {
            let payment = transaction("new", "inbound", "completed");
            let event = WalletIncomingPaymentEvent {
                transaction: payment.clone(),
                status: WalletStatus {
                    provider_name: "Lexe".to_string(),
                    balance: 21,
                    spendable_balance: 20,
                    lightning_balance: 21,
                    onchain_balance: 0,
                },
                transactions: vec![payment],
            };

            let json = serde_json::to_value(event).expect("event serializes");
            assert_eq!(json["transaction"]["id"], "new");
            assert_eq!(json["status"]["spendableBalance"], 20);
            assert_eq!(json["transactions"][0]["id"], "new");
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

    pub fn start_wallet_reconciler(_app: tauri::AppHandle) {}

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
    disabled_async_command!(wallet_set_polling_enabled(enabled: bool) -> ());
    #[tauri::command]
    pub fn wallet_parse_zap_events(
        events: Vec<serde_json::Value>,
        allowed_recipient_pubkeys: Option<Vec<String>>,
    ) -> Vec<serde_json::Value> {
        let _ = (events, allowed_recipient_pubkeys);
        Vec::new()
    }
    disabled_async_command!(
        wallet_get_recipient_offer(recipient_pubkey: String) -> serde_json::Value
    );
    disabled_async_command!(
        wallet_get_pending_profile_zap(
            recipient_pubkey: String,
            target_event_id: Option<String>,
        ) -> serde_json::Value
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

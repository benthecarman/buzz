/// Whether this desktop binary was built with the experimental Bitcoin wallet.
#[tauri::command]
pub fn bitcoin_compile_enabled() -> bool {
    cfg!(feature = "bitcoin")
}

#[cfg(feature = "bitcoin")]
pub(crate) mod enabled {
    mod zap_commands;
    use std::{
        collections::HashSet,
        sync::{Arc, OnceLock},
    };
    pub use zap_commands::{
        wallet_get_pending_profile_zap, wallet_get_recipient_offer,
        wallet_list_placeholder_message_zaps, wallet_poll_updates, wallet_send_agent_runtime_zap,
        wallet_send_profile_zap,
    };

    /// Start recipient-side runtime-payment reconciliation for the lifetime of
    /// the application. This task is intentionally owned by Tauri rather than
    /// a React settings panel: paid Agents must mint verified deposits while
    /// the app is open regardless of which screen is visible.
    pub fn start_agent_runtime_wallet_reconciler(app: AppHandle) {
        tauri::async_runtime::spawn(async move {
            let mut consecutive_failures = 0u32;
            loop {
                let state = app.state::<AppState>();
                match zap_commands::reconcile_agent_runtime_background_once(&app, &state).await {
                    Ok(_) => consecutive_failures = 0,
                    Err(error) => {
                        consecutive_failures = consecutive_failures.saturating_add(1);
                        tracing::warn!(
                            code = error.code,
                            error = %error.message,
                            "background Agent runtime payment reconciliation failed"
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
    }

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
                WalletOfferPublicationResult, WalletPaymentResult, WalletProfileZapResult,
                WalletRecipientOffer, WalletSendRequest, WalletStatus, WalletTransactionPage,
            },
            offer_conformance::OfferPublicationTrace,
            provider::{WalletPaymentMatch, WalletProvider},
            send::{SendAttempt, SendAttemptState, SendAttemptStore},
            zap::{
                build_offer_announcement, build_offer_withdrawal, recipient_offer,
                validate_offer_event, ZapAttempt, ZapAttemptStore,
            },
            WalletManager,
        },
    };

    /// How long a `Paying` attempt is reconciled against the provider before
    /// it is declared failed. Generous on purpose: Lightning HTLC expiry
    /// resolves in-flight payments within hours, so a payment the provider
    /// still cannot see a full day after dispatch never left the wallet.
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

    /// Reuse or create an agent-scoped offer in the user's wallet, then
    /// broadcast an announcement signed by the agent identity.
    async fn publish_managed_agent_offer(
        state: &AppState,
        agent_keys: &nostr::Keys,
        user_keys: &nostr::Keys,
        user_provider: &Arc<dyn WalletProvider>,
        relay_api_base_urls: &[String],
    ) -> Result<Vec<String>, WalletError> {
        let agent_pubkey = agent_keys.public_key().to_hex();
        let offer = user_provider.scoped_offer(agent_pubkey, false).await?;
        publish_offer(state, agent_keys, user_keys, relay_api_base_urls, &offer).await
    }

    /// Give every existing managed agent a distinct offer from the user's
    /// wallet. This is best-effort per agent: enabling the user's wallet must
    /// not fail after it has already published because one agent key or relay
    /// is temporarily unavailable.
    async fn provision_managed_agent_offers(
        app: &AppHandle,
        state: &AppState,
        user_keys: &nostr::Keys,
        user_provider: &Arc<dyn WalletProvider>,
        relay_api_base_urls: &[String],
    ) -> Vec<String> {
        let records = {
            let Ok(_guard) = state.managed_agents_store_lock.lock() else {
                return vec!["managed agents: agent store lock is unavailable".to_string()];
            };
            match crate::managed_agents::load_managed_agents(app) {
                Ok(records) => records,
                Err(error) => return vec![format!("managed agents: {error}")],
            }
        };

        let results = join_all(records.into_iter().map(|record| async move {
            let pubkey = record.pubkey;
            let keys = nostr::Keys::parse(record.private_key_nsec.trim()).map_err(|error| {
                WalletError::unavailable(format!("load agent {pubkey} signing key: {error}"))
            })?;
            if keys.public_key().to_hex() != pubkey {
                return Err(WalletError::unavailable(format!(
                    "agent {pubkey} signing key does not match its public key"
                )));
            }
            publish_managed_agent_offer(state, &keys, user_keys, user_provider, relay_api_base_urls)
                .await
                .map(|warnings| (pubkey, warnings))
        }))
        .await;

        results
            .into_iter()
            .flat_map(|result| match result {
                Ok((pubkey, warnings)) => warnings
                    .into_iter()
                    .map(move |warning| format!("agent {pubkey}: {warning}"))
                    .collect::<Vec<_>>(),
                Err(error) => vec![error.message],
            })
            .collect()
    }

    /// Withdraw every managed agent's replaceable offer when the owner turns
    /// the wallet feature off. A failed agent withdrawal is reported alongside
    /// the existing relay publication warnings and does not prevent the other
    /// identities from being withdrawn.
    async fn withdraw_managed_agent_offers(
        app: &AppHandle,
        state: &AppState,
        relay_api_base_urls: &[String],
    ) -> Vec<String> {
        let records = {
            let Ok(_guard) = state.managed_agents_store_lock.lock() else {
                return vec!["managed agents: agent store lock is unavailable".to_string()];
            };
            match crate::managed_agents::load_managed_agents(app) {
                Ok(records) => records,
                Err(error) => return vec![format!("managed agents: {error}")],
            }
        };

        join_all(records.into_iter().map(|record| async move {
            let pubkey = record.pubkey;
            let result = async {
                let keys = nostr::Keys::parse(record.private_key_nsec.trim()).map_err(|error| {
                    WalletError::unavailable(format!("load signing key: {error}"))
                })?;
                if keys.public_key().to_hex() != pubkey {
                    return Err(WalletError::unavailable(
                        "signing key does not match its public key",
                    ));
                }
                let event = build_offer_withdrawal()
                    .sign_with_keys(&keys)
                    .map_err(|error| {
                        WalletError::new(
                            "relay_publish_failed",
                            format!("sign BOLT12 offer withdrawal: {error}"),
                        )
                    })?;
                publish_wallet_event(state, &keys, None, relay_api_base_urls, &event).await
            }
            .await;
            (pubkey, result)
        }))
        .await
        .into_iter()
        .flat_map(|(pubkey, result)| match result {
            Ok(warnings) => warnings
                .into_iter()
                .map(move |warning| format!("agent {pubkey}: {warning}"))
                .collect::<Vec<_>>(),
            Err(error) => vec![format!("agent {pubkey}: {}", error.message)],
        })
        .collect()
    }

    /// Create an offer from the user's wallet and publish it for a newly
    /// created managed agent. The caller supplies the agent signing keys.
    pub(crate) async fn provision_new_managed_agent_offer(
        app: &AppHandle,
        state: &AppState,
        keys: &nostr::Keys,
        relay_urls: Vec<String>,
    ) -> Result<Vec<String>, String> {
        let relay_api_base_urls =
            wallet_relay_api_base_urls(&relay_api_base_url_with_override(state), Some(relay_urls));
        let app_data_dir = app_data_dir(app).map_err(|error| error.message)?;
        let user_keys = state.signing_keys()?;
        let user_provider = wallet_manager()
            .provider_for(&user_keys, &app_data_dir)
            .await
            .map_err(|error| error.message)?;
        user_provider
            .provision()
            .await
            .map_err(|error| error.message)?;
        publish_managed_agent_offer(
            state,
            keys,
            &user_keys,
            &user_provider,
            &relay_api_base_urls,
        )
        .await
        .map_err(|error| error.message)
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
    ) -> Result<Option<String>, WalletError> {
        let (Some(target_event_id), Some(target_event_kind)) = (
            attempt.target_event_id.as_deref(),
            attempt.target_event_kind,
        ) else {
            return Ok(None);
        };
        let active_relay = relay_api_base_url_with_override(state);
        let events = query_relay_at_with_keys(
            state,
            &active_relay,
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
                let channel_id = match channel_override {
                    Some(channel_id) => Some(channel_id.to_string()),
                    None => zap_target_channel_id(state, keys, attempt).await?,
                };
                let event =
                    store.prepare_placeholder_proof(attempt, keys, channel_id.as_deref())?;
                let active_relay = relay_api_base_url_with_override(state);
                submit_signed_event_at_with_keys(&event, state, &active_relay, keys)
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
        provider.provision().await?;
        let status = provider.status().await?;
        let offer = provider.offer(false).await?;
        let relay_api_base_urls =
            wallet_relay_api_base_urls(&relay_api_base_url_with_override(&state), relay_urls);
        let mut publication_warnings =
            publish_offer(&state, &keys, &keys, &relay_api_base_urls, &offer).await?;
        publication_warnings.extend(
            super::super::wallet_nwc::publish_nwc_info(&state, &keys, &relay_api_base_urls, true)
                .await?,
        );
        publication_warnings.extend(
            provision_managed_agent_offers(&app, &state, &keys, &provider, &relay_api_base_urls)
                .await,
        );
        Ok(WalletEnableResult {
            status,
            publication_warnings,
        })
    }

    #[tauri::command]
    pub async fn wallet_disable(
        app: AppHandle,
        state: State<'_, AppState>,
        relay_urls: Option<Vec<String>>,
    ) -> Result<WalletOfferPublicationResult, WalletError> {
        let paid_agents = crate::managed_agents::load_managed_agents(&app)
            .map_err(WalletError::unavailable)?
            .into_iter()
            .filter(|agent| agent.price_per_minute_sats.is_some())
            .map(|agent| agent.name)
            .collect::<Vec<_>>();
        if !paid_agents.is_empty() {
            return Err(WalletError::new(
                "wallet_in_use",
                format!(
                    "Disable paid runtime pricing before disabling the wallet: {}",
                    paid_agents.join(", ")
                ),
            ));
        }
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
        publication_warnings
            .extend(withdraw_managed_agent_offers(&app, &state, &relay_api_base_urls).await);
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
            PAYING_ATTEMPT_GRACE_MS,
        };

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

    pub fn start_agent_runtime_wallet_reconciler(_app: tauri::AppHandle) {}

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
        wallet_get_pending_profile_zap(
            recipient_pubkey: String,
            target_event_id: Option<String>,
        ) -> serde_json::Value
    );
    disabled_async_command!(wallet_list_placeholder_message_zaps() -> serde_json::Value);
    disabled_async_command!(
        wallet_send_profile_zap(request: serde_json::Value) -> serde_json::Value
    );
    disabled_async_command!(
        wallet_send_agent_runtime_zap(request: serde_json::Value) -> serde_json::Value
    );

    #[tauri::command]
    pub fn wallet_reveal_recovery_phrase() -> Result<String, WalletDisabledError> {
        Err(disabled())
    }
}

#[cfg(not(feature = "bitcoin"))]
pub use disabled::*;

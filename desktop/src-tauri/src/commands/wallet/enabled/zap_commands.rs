use std::collections::HashSet;

use buzz_core_pkg::{
    agent_runtime_payment::{RuntimeDeposit, RuntimePricing, RuntimePurchase, VERSION},
    kind::{KIND_AGENT_RUNTIME_DEPOSIT, KIND_AGENT_RUNTIME_PRICING, KIND_BOLT12_ZAP},
};
use nostr::{Event, EventBuilder, JsonUtil, Kind, Tag};
use tauri::{AppHandle, State};

use super::{
    app_data_dir, now_ms, paying_attempt_expired, payment_error, profile_payment_result,
    resolve_recipient_offer, wallet_manager, zap_target_channel_id,
};
use crate::{
    app_state::AppState,
    relay::{
        query_relay_at_with_keys, relay_api_base_url_with_override,
        submit_signed_event_at_with_keys, submit_signed_event_with_keys,
    },
    wallet::{
        models::{
            WalletAgentRuntimeZapRequest, WalletError, WalletOfferSendRequest,
            WalletProfileZapDraft, WalletProfileZapRequest, WalletProfileZapResult,
            WalletRecipientOffer,
        },
        provider::WalletPaymentMatch,
        zap::{
            parse_tagged_zap_event, recipient_offer, ZapAttempt, ZapAttemptState, ZapAttemptStore,
        },
    },
};

fn exact_tag<'a>(event: &'a Event, name: &str) -> Option<&'a str> {
    let values = event
        .tags
        .iter()
        .filter_map(|tag| {
            let parts = tag.as_slice();
            (parts.len() == 2 && parts[0].as_str() == name).then(|| parts[1].as_str())
        })
        .collect::<Vec<_>>();
    match values.as_slice() {
        [value] => Some(*value),
        _ => None,
    }
}

fn runtime_deposit_path(
    app: &AppHandle,
    agent_pubkey: &str,
    intent_id: &str,
) -> Result<std::path::PathBuf, WalletError> {
    Ok(app_data_dir(app)?
        .join("wallet")
        .join("agent-runtime-deposits")
        .join(agent_pubkey)
        .join(format!("{intent_id}.json")))
}

fn runtime_deposit_ack_path(
    app: &AppHandle,
    agent_pubkey: &str,
    intent_id: &str,
) -> Result<std::path::PathBuf, WalletError> {
    Ok(runtime_deposit_path(app, agent_pubkey, intent_id)?.with_extension("accepted"))
}

fn persist_runtime_deposit_ack(
    app: &AppHandle,
    agent_pubkey: &str,
    intent_id: &str,
    event_id: &str,
) -> Result<(), WalletError> {
    use std::io::Write;

    let path = runtime_deposit_ack_path(app, agent_pubkey, intent_id)?;
    let mut file = atomic_write_file::AtomicWriteFile::open(path)
        .map_err(|error| WalletError::unavailable(format!("open deposit receipt: {error}")))?;
    file.write_all(event_id.as_bytes())
        .map_err(|error| WalletError::unavailable(format!("write deposit receipt: {error}")))?;
    file.commit()
        .map_err(|error| WalletError::unavailable(format!("commit deposit receipt: {error}")))
}

fn persist_runtime_deposit(
    app: &AppHandle,
    agent_pubkey: &str,
    intent_id: &str,
    event: &Event,
) -> Result<Event, WalletError> {
    use std::io::Write;

    let path = runtime_deposit_path(app, agent_pubkey, intent_id)?;
    let parent = path
        .parent()
        .ok_or_else(|| WalletError::unavailable("runtime deposit path has no parent directory"))?;
    std::fs::create_dir_all(parent)
        .map_err(|error| WalletError::unavailable(format!("create deposit store: {error}")))?;
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    match options.open(&path) {
        Ok(mut file) => {
            file.write_all(event.as_json().as_bytes())
                .map_err(|error| {
                    WalletError::unavailable(format!("write deposit store: {error}"))
                })?;
            file.sync_all().map_err(|error| {
                WalletError::unavailable(format!("sync deposit store: {error}"))
            })?;
            Ok(event.clone())
        }
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            let json = std::fs::read_to_string(path).map_err(|error| {
                WalletError::unavailable(format!("read winning deposit: {error}"))
            })?;
            Event::from_json(json).map_err(|error| {
                WalletError::unavailable(format!("decode winning deposit: {error}"))
            })
        }
        Err(error) => Err(WalletError::unavailable(format!(
            "create deposit store: {error}"
        ))),
    }
}

async fn reconcile_agent_runtime_deposits(
    app: &AppHandle,
    state: &AppState,
) -> Result<bool, WalletError> {
    let agents = crate::managed_agents::load_managed_agents(app)
        .map_err(WalletError::unavailable)?
        .into_iter()
        .collect::<Vec<_>>();
    if agents.is_empty() {
        return Ok(false);
    }

    let relay = relay_api_base_url_with_override(state);
    let mut published_any = false;
    for agent in agents {
        let agent_keys = nostr::Keys::parse(&agent.private_key_nsec).map_err(|error| {
            WalletError::unavailable(format!("load agent signing key: {error}"))
        })?;
        let zaps = query_all_relay_events(
            state,
            &relay,
            serde_json::json!({
                "kinds": [KIND_BOLT12_ZAP],
                "#p": [agent.pubkey],
            }),
            &agent_keys,
            agent.auth_tag.as_deref(),
        )
        .await?;

        for zap in zaps {
            let Ok(raw_zap) = serde_json::to_value(&zap) else {
                continue;
            };
            let Ok(verified_zap) = parse_tagged_zap_event(&raw_zap) else {
                continue;
            };
            if verified_zap.recipient_pubkey != agent.pubkey
                || verified_zap.target_event_id.is_some()
            {
                continue;
            }
            let intent_id = verified_zap.intent_event_id.as_str();
            let Some(description) = exact_tag(&zap, "description") else {
                continue;
            };
            let Ok(intent) = Event::from_json(description) else {
                continue;
            };
            let amount_sats = verified_zap.amount;
            let Some(offer_event_json) = exact_tag(&intent, "offer_event") else {
                continue;
            };
            let Ok(offer_event) = Event::from_json(offer_event_json) else {
                continue;
            };
            if recipient_offer(&offer_event, &agent.pubkey).is_err() {
                continue;
            }
            let Some(purchase_json) = exact_tag(&intent, "agent_runtime_purchase") else {
                continue;
            };
            // The purchase pins the exact agent-signed pricing the payer paid
            // against, so attestation needs no live party and no relay fetch.
            // A pinned rate is honored even if the agent has since repriced:
            // the agent's own signature advertised it, and there is no refund
            // path — crediting at the signed rate is the bounded-harm choice.
            let Ok(purchase) = validate_agent_runtime_purchase(purchase_json, &agent.pubkey) else {
                continue;
            };
            let Ok(pricing_event) = Event::from_json(purchase.pricing_event.to_string()) else {
                continue;
            };
            if purchase.amount_sats != amount_sats {
                continue;
            }

            let path = runtime_deposit_path(app, &agent.pubkey, intent_id)?;
            let deposit_event = match std::fs::read_to_string(&path) {
                Ok(json) => Event::from_json(json).map_err(|error| {
                    WalletError::unavailable(format!("decode persisted deposit: {error}"))
                })?,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    let credit_ms = u64::from(purchase.pack_minutes)
                        .checked_mul(60_000)
                        .ok_or_else(|| {
                            WalletError::new(
                                "runtime_quote_invalid",
                                "runtime credit amount overflow",
                            )
                        })?;
                    let deposit = RuntimeDeposit {
                        version: VERSION,
                        pack_minutes: purchase.pack_minutes,
                        credit_ms,
                        price_per_minute_sats: purchase.price_per_minute_sats,
                        amount_sats,
                    };
                    deposit.validate().map_err(|error| {
                        WalletError::new("runtime_quote_invalid", error.to_string())
                    })?;
                    let event = EventBuilder::new(
                        Kind::Custom(KIND_AGENT_RUNTIME_DEPOSIT as u16),
                        serde_json::to_string(&deposit).map_err(|error| {
                            WalletError::unavailable(format!("encode runtime deposit: {error}"))
                        })?,
                    )
                    .tags([
                        Tag::parse(["p", intent.pubkey.to_hex().as_str()])
                            .map_err(|error| WalletError::unavailable(error.to_string()))?,
                        Tag::parse(["h", purchase.channel_id.as_str()])
                            .map_err(|error| WalletError::unavailable(error.to_string()))?,
                        Tag::parse(["pricing", pricing_event.id.to_hex().as_str()])
                            .map_err(|error| WalletError::unavailable(error.to_string()))?,
                        Tag::parse(["zap", zap.id.to_hex().as_str()])
                            .map_err(|error| WalletError::unavailable(error.to_string()))?,
                        Tag::parse(["zap_intent", intent_id])
                            .map_err(|error| WalletError::unavailable(error.to_string()))?,
                    ])
                    .sign_with_keys(&agent_keys)
                    .map_err(|error| {
                        WalletError::unavailable(format!("sign runtime deposit: {error}"))
                    })?;
                    persist_runtime_deposit(app, &agent.pubkey, intent_id, &event)?
                }
                Err(error) => {
                    return Err(WalletError::unavailable(format!(
                        "read runtime deposit: {error}"
                    )))
                }
            };
            if exact_tag(&deposit_event, "zap") != Some(zap.id.to_hex().as_str()) {
                // The intent is the idempotency boundary. If duplicate proof
                // events exist, retain the receipt that won the atomic local
                // deposit write and ignore the others.
                continue;
            }
            let persisted: RuntimeDeposit =
                serde_json::from_str(&deposit_event.content).map_err(|error| {
                    WalletError::unavailable(format!("decode persisted deposit content: {error}"))
                })?;
            if deposit_event.verify().is_err()
                || deposit_event.pubkey != agent_keys.public_key()
                || deposit_event.kind != Kind::Custom(KIND_AGENT_RUNTIME_DEPOSIT as u16)
                || exact_tag(&deposit_event, "p") != Some(intent.pubkey.to_hex().as_str())
                || exact_tag(&deposit_event, "h") != Some(purchase.channel_id.as_str())
                || exact_tag(&deposit_event, "pricing") != Some(pricing_event.id.to_hex().as_str())
                || exact_tag(&deposit_event, "zap") != Some(zap.id.to_hex().as_str())
                || exact_tag(&deposit_event, "zap_intent") != Some(intent_id)
                || persisted.validate().is_err()
                || persisted.pack_minutes != purchase.pack_minutes
                || persisted.credit_ms != u64::from(purchase.pack_minutes) * 60_000
                || persisted.price_per_minute_sats != purchase.price_per_minute_sats
                || persisted.amount_sats != amount_sats
            {
                return Err(WalletError::unavailable(
                    "persisted runtime deposit conflicts with its verified payment",
                ));
            }
            let deposit_event_id = deposit_event.id.to_hex();
            if std::fs::read_to_string(runtime_deposit_ack_path(app, &agent.pubkey, intent_id)?)
                .ok()
                .as_deref()
                == Some(deposit_event_id.as_str())
            {
                continue;
            }
            submit_signed_event_with_keys(
                &deposit_event,
                state,
                &agent_keys,
                agent.auth_tag.as_deref(),
            )
            .await
            .map_err(WalletError::unavailable)?;
            persist_runtime_deposit_ack(app, &agent.pubkey, intent_id, &deposit_event_id)?;
            tracing::info!(
                agent_pubkey = %agent.pubkey,
                zap_intent = %intent_id,
                amount_sats,
                credit_ms = persisted.credit_ms,
                reconciliation_latency_ms = now_ms().saturating_sub(
                    zap.created_at.as_secs().saturating_mul(1_000)
                ),
                "Agent runtime deposit accepted"
            );
            published_any = true;
        }
    }
    Ok(published_any)
}

const RELAY_HISTORY_PAGE_SIZE: usize = 250;

async fn query_all_relay_events(
    state: &AppState,
    relay: &str,
    base_filter: serde_json::Value,
    keys: &nostr::Keys,
    auth_tag: Option<&str>,
) -> Result<Vec<Event>, WalletError> {
    let mut until = None;
    let mut seen = HashSet::new();
    let mut events = Vec::new();
    loop {
        let mut filter = base_filter.clone();
        let object = filter.as_object_mut().ok_or_else(|| {
            WalletError::unavailable("runtime relay filter must be a JSON object")
        })?;
        object.insert(
            "limit".into(),
            serde_json::Value::from(RELAY_HISTORY_PAGE_SIZE),
        );
        if let Some(cursor) = until {
            object.insert("until".into(), serde_json::Value::from(cursor));
        }
        let page =
            query_relay_at_with_keys(state, relay, std::slice::from_ref(&filter), keys, auth_tag)
                .await
                .map_err(WalletError::unavailable)?;
        if page.is_empty() {
            break;
        }
        let page_len = page.len();
        let oldest = page
            .iter()
            .map(|event| event.created_at.as_secs())
            .min()
            .ok_or_else(|| WalletError::unavailable("runtime relay page had no timestamp"))?;
        let mut inserted = 0usize;
        for event in page {
            if seen.insert(event.id) {
                inserted += 1;
                events.push(event);
            }
        }
        if page_len < RELAY_HISTORY_PAGE_SIZE {
            break;
        }
        if until == Some(oldest) && inserted == 0 {
            return Err(WalletError::unavailable(
                "runtime relay paging cannot advance across one dense timestamp",
            ));
        }
        until = Some(oldest);
    }
    Ok(events)
}

pub(crate) async fn reconcile_wallet_background_once(
    app: &AppHandle,
    state: &AppState,
) -> Result<bool, WalletError> {
    let reconciled_zaps = reconcile_paying_zap_attempts(app, state).await?;
    let published_proofs = reconcile_pending_zap_proofs(app, state).await?;
    let deposited = reconcile_agent_runtime_deposits(app, state).await?;
    Ok(reconciled_zaps || published_proofs || deposited)
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
        let personal_note = if attempt.runtime_purchase_json.is_some() {
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

        let runtime_channel = attempt
            .runtime_purchase_json
            .as_deref()
            .and_then(|json| serde_json::from_str::<RuntimePurchase>(json).ok())
            .map(|purchase| purchase.channel_id);
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
/// Validate purchase terms against the agent-signed pricing they pin.
///
/// The pricing event must be signed by the Agent the zap pays, advertise an
/// enabled rate equal to the purchase's, and offer the purchased pack. The
/// arithmetic is re-checked by `RuntimePurchase::validate`.
fn validate_agent_runtime_purchase(
    purchase_json: &str,
    agent_pubkey: &str,
) -> Result<RuntimePurchase, WalletError> {
    let purchase: RuntimePurchase = serde_json::from_str(purchase_json)
        .map_err(|error| WalletError::new("runtime_purchase_invalid", error.to_string()))?;
    purchase
        .validate()
        .map_err(|error| WalletError::new("runtime_purchase_invalid", error.to_string()))?;
    let pricing_event = Event::from_json(purchase.pricing_event.to_string())
        .map_err(|error| WalletError::new("runtime_purchase_invalid", error.to_string()))?;
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
        || pricing.rate_sats_per_minute != Some(purchase.price_per_minute_sats)
        || !pricing
            .runtime_packs_minutes
            .contains(&purchase.pack_minutes)
    {
        return Err(WalletError::new(
            "runtime_purchase_invalid",
            "purchase terms do not match the agent's signed pricing",
        ));
    }
    Ok(purchase)
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
    let pricing: RuntimePricing = serde_json::from_str(&pricing_event.content)
        .map_err(|error| WalletError::new("runtime_purchase_invalid", error.to_string()))?;
    let rate = pricing.rate_sats_per_minute.ok_or_else(|| {
        WalletError::new("runtime_purchase_invalid", "agent pricing is disabled")
    })?;
    let amount_sats = rate
        .checked_mul(u64::from(request.pack_minutes))
        .ok_or_else(|| WalletError::new("runtime_purchase_invalid", "purchase amount overflow"))?;
    let purchase = RuntimePurchase {
        version: VERSION,
        channel_id: request.channel_id,
        pack_minutes: request.pack_minutes,
        price_per_minute_sats: rate,
        amount_sats,
        pricing_event: serde_json::to_value(&pricing_event)
            .map_err(|error| WalletError::new("runtime_purchase_invalid", error.to_string()))?,
    };
    let purchase_json = serde_json::to_string(&purchase)
        .map_err(|error| WalletError::new("runtime_purchase_invalid", error.to_string()))?;
    let purchase = validate_agent_runtime_purchase(&purchase_json, &request.agent_pubkey)?;
    let relay = relay_api_base_url_with_override(&state);
    let recipient = resolve_recipient_offer(&state, &keys, &relay, &request.agent_pubkey).await?;
    let payment_request = WalletProfileZapRequest {
        recipient_pubkey: request.agent_pubkey,
        amount: purchase.amount_sats,
        comment: None,
        idempotency_key: request.idempotency_key,
        target_event_id: None,
        target_event_kind: None,
    };
    wallet_send_zap_attempt(
        app,
        state,
        payment_request,
        Some((purchase, recipient, purchase_json)),
    )
    .await
}

async fn wallet_send_zap_attempt(
    app: AppHandle,
    state: State<'_, AppState>,
    request: WalletProfileZapRequest,
    prepared_runtime: Option<(RuntimePurchase, WalletRecipientOffer, String)>,
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
    let purchase_json = prepared_runtime.as_ref().map(|(_, _, json)| json.clone());
    let runtime_channel = prepared_runtime
        .as_ref()
        .map(|(purchase, _, _)| purchase.channel_id.clone());
    let mut attempt = match store.load(&request.idempotency_key)? {
        Some(attempt)
            if attempt.recipient_pubkey == request.recipient_pubkey
                && attempt.amount == request.amount
                && attempt.comment == comment
                && attempt.target_event_id == request.target_event_id
                && attempt.target_event_kind == request.target_event_kind
                && attempt.runtime_purchase_json == purchase_json =>
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
            let mut attempt = if let Some((_, recipient, purchase_json)) = prepared_runtime {
                ZapAttempt::prepare_agent_runtime(
                    request.idempotency_key,
                    recipient,
                    request.amount,
                    purchase_json,
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

    let personal_note = if attempt.runtime_purchase_json.is_some() {
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

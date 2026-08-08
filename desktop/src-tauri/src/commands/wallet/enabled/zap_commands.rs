use std::{collections::HashSet, sync::Arc};

use buzz_core_pkg::{
    agent_runtime_payment::{RuntimeDeposit, RuntimeQuote, RuntimeReservationResponse, VERSION},
    kind::{
        KIND_AGENT_RUNTIME_DEPOSIT, KIND_AGENT_RUNTIME_RESPONSE, KIND_BOLT12_ZAP,
        KIND_BOLT12_ZAP_INTENT,
    },
};
use nostr::{nips::nip44, Event, EventBuilder, JsonUtil, Kind, Tag};
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
        canonical_offer_id,
        models::{
            WalletAgentRuntimeZapRequest, WalletError, WalletOfferSendRequest,
            WalletPlaceholderMessageZap, WalletProfileZapDraft, WalletProfileZapRequest,
            WalletProfileZapResult, WalletRecipientOffer,
        },
        provider::{WalletPaymentMatch, WalletProvider},
        zap::{recipient_offer, ZapAttempt, ZapAttemptState, ZapAttemptStore},
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
    provider: &Arc<dyn WalletProvider>,
) -> Result<bool, WalletError> {
    let agents = crate::managed_agents::load_managed_agents(app)
        .map_err(WalletError::unavailable)?
        .into_iter()
        .collect::<Vec<_>>();
    if agents.is_empty() {
        return Ok(false);
    }

    // Provider cursors are opaque. Follow them to completion and reject a
    // repeated cursor instead of silently treating a partial history as
    // authoritative. Deposit files make replaying old completed entries cheap
    // and idempotent.
    let mut transactions = Vec::new();
    let mut cursor = None;
    let mut seen_cursors = HashSet::new();
    loop {
        let page = provider.transactions(cursor.clone(), 100, false).await?;
        transactions.extend(page.transactions);
        let Some(next) = page.next_cursor else {
            break;
        };
        if !seen_cursors.insert(next.clone()) {
            return Err(WalletError::unavailable(
                "wallet transaction pagination repeated a cursor",
            ));
        }
        cursor = Some(next);
    }
    let candidates = transactions
        .into_iter()
        .filter(|transaction| {
            transaction.direction == "inbound"
                && transaction.status == "completed"
                && transaction.amount.is_some()
                && transaction.offer_id.is_some()
                && transaction
                    .payer_note
                    .as_deref()
                    .is_some_and(|note| note.starts_with("nostr:nipB1:"))
        })
        .collect::<Vec<_>>();
    if candidates.is_empty() {
        return Ok(false);
    }

    let relay = relay_api_base_url_with_override(state);
    let mut published_any = false;
    for agent in agents {
        let offer = provider.scoped_offer(agent.pubkey.clone(), false).await?;
        let Some(expected_offer_id) = canonical_offer_id(&offer) else {
            continue;
        };
        let matching_transactions = candidates
            .iter()
            .filter(|transaction| transaction.offer_id.as_deref() == Some(&expected_offer_id))
            .collect::<Vec<_>>();
        if matching_transactions.is_empty() {
            continue;
        }
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

        for transaction in matching_transactions {
            let Some(intent_id) = transaction
                .payer_note
                .as_deref()
                .and_then(|note| note.strip_prefix("nostr:nipB1:"))
            else {
                continue;
            };
            if intent_id.len() != 64 {
                continue;
            }
            let operation_lock = wallet_manager()
                .operation_lock(&agent.pubkey, intent_id)
                .await;
            let _operation_guard = operation_lock.lock().await;
            let Some(zap) = zaps.iter().find(|zap| {
                exact_tag(zap, "description")
                    .and_then(|json| Event::from_json(json).ok())
                    .is_some_and(|intent| intent.id.to_hex() == intent_id)
            }) else {
                continue;
            };
            if zap.verify().is_err() {
                continue;
            }
            let Some(description) = exact_tag(zap, "description") else {
                continue;
            };
            let Ok(intent) = Event::from_json(description) else {
                continue;
            };
            if intent.verify().is_err()
                || intent.kind != Kind::Custom(KIND_BOLT12_ZAP_INTENT as u16)
                || intent.id.to_hex() != intent_id
                || zap.pubkey != intent.pubkey
                || exact_tag(&intent, "p") != Some(agent.pubkey.as_str())
            {
                continue;
            }
            let Some(amount_sats) = exact_tag(&intent, "amount")
                .and_then(|amount| amount.parse::<u64>().ok())
                .filter(|amount_msats| amount_msats % 1_000 == 0)
                .map(|amount_msats| amount_msats / 1_000)
            else {
                continue;
            };
            if transaction.amount != Some(amount_sats) {
                continue;
            }
            let Some(offer_event_json) = exact_tag(&intent, "offer_event") else {
                continue;
            };
            let Ok(offer_event) = Event::from_json(offer_event_json) else {
                continue;
            };
            let Ok(recipient) = recipient_offer(&offer_event, &agent.pubkey) else {
                continue;
            };
            if recipient.offer != offer {
                continue;
            }
            let Some(quote_event_json) = exact_tag(&intent, "agent_runtime_quote") else {
                continue;
            };
            let Ok(quote_event) = Event::from_json(quote_event_json) else {
                continue;
            };
            if quote_event.verify().is_err()
                || quote_event.kind != Kind::Custom(KIND_AGENT_RUNTIME_RESPONSE as u16)
                || quote_event.pubkey != agent_keys.public_key()
                || exact_tag(&quote_event, "p") != Some(intent.pubkey.to_hex().as_str())
            {
                continue;
            }
            let Ok(plaintext) = nip44::decrypt(
                agent_keys.secret_key(),
                &intent.pubkey,
                &quote_event.content,
            ) else {
                continue;
            };
            let Ok(RuntimeReservationResponse::PaymentRequired { quote }) =
                serde_json::from_str::<RuntimeReservationResponse>(&plaintext)
            else {
                continue;
            };
            if quote.validate().is_err()
                || quote.agent_pubkey != agent.pubkey
                || quote.payer_pubkey != intent.pubkey.to_hex()
                || quote.amount_sats != amount_sats
                || serde_json::to_value(&offer_event).ok() != Some(quote.offer_event.clone())
                || exact_tag(&quote_event, "expiration")
                    .and_then(|value| value.parse::<u64>().ok())
                    != Some(quote.expires_at)
                || exact_tag(&quote_event, "encryption") != Some("nip44_v2")
                // A delayed reconciliation is valid, but an intent or provider
                // payment created after the signed deadline is an expired-price
                // replay and must never mint credit.
                || quote_event.created_at.as_secs() > quote.expires_at
                || quote.expires_at
                    > quote_event
                        .created_at
                        .as_secs()
                        .saturating_add(5 * 60)
                || intent.created_at < quote_event.created_at
                || intent.created_at.as_secs() > quote.expires_at
                || transaction.created_at_ms / 1_000 > quote.expires_at
            {
                continue;
            }

            let path = runtime_deposit_path(app, &agent.pubkey, intent_id)?;
            let deposit_event = match std::fs::read_to_string(&path) {
                Ok(json) => Event::from_json(json).map_err(|error| {
                    WalletError::unavailable(format!("decode persisted deposit: {error}"))
                })?,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    let credit_ms = u64::from(quote.pack_minutes)
                        .checked_mul(60_000)
                        .ok_or_else(|| {
                            WalletError::new(
                                "runtime_quote_invalid",
                                "runtime credit amount overflow",
                            )
                        })?;
                    let deposit = RuntimeDeposit {
                        version: VERSION,
                        pack_minutes: quote.pack_minutes,
                        credit_ms,
                        price_per_minute_sats: quote.price_per_minute_sats,
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
                        Tag::parse(["h", quote.channel_id.as_str()])
                            .map_err(|error| WalletError::unavailable(error.to_string()))?,
                        Tag::parse(["quote", quote_event.id.to_hex().as_str()])
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
            let persisted: RuntimeDeposit =
                serde_json::from_str(&deposit_event.content).map_err(|error| {
                    WalletError::unavailable(format!("decode persisted deposit content: {error}"))
                })?;
            if deposit_event.verify().is_err()
                || deposit_event.pubkey != agent_keys.public_key()
                || deposit_event.kind != Kind::Custom(KIND_AGENT_RUNTIME_DEPOSIT as u16)
                || exact_tag(&deposit_event, "p") != Some(intent.pubkey.to_hex().as_str())
                || exact_tag(&deposit_event, "h") != Some(quote.channel_id.as_str())
                || exact_tag(&deposit_event, "quote") != Some(quote_event.id.to_hex().as_str())
                || exact_tag(&deposit_event, "zap") != Some(zap.id.to_hex().as_str())
                || exact_tag(&deposit_event, "zap_intent") != Some(intent_id)
                || persisted.validate().is_err()
                || persisted.pack_minutes != quote.pack_minutes
                || persisted.credit_ms != u64::from(quote.pack_minutes) * 60_000
                || persisted.price_per_minute_sats != quote.price_per_minute_sats
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
                reconciliation_latency_ms = now_ms().saturating_sub(transaction.created_at_ms),
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

pub(crate) async fn reconcile_agent_runtime_background_once(
    app: &AppHandle,
    state: &AppState,
) -> Result<bool, WalletError> {
    // Retain reconciliation after pricing is disabled: an inbound payment may
    // have settled just before the operator removed the active rate.
    if crate::managed_agents::load_managed_agents(app)
        .map_err(WalletError::unavailable)?
        .is_empty()
    {
        return Ok(false);
    }
    let keys = state.signing_keys().map_err(WalletError::unavailable)?;
    let provider = wallet_manager()
        .provider_for(&keys, &app_data_dir(app)?)
        .await?;
    provider.poll_updates().await?;
    reconcile_agent_runtime_deposits(app, state, &provider).await
}

#[tauri::command]
pub async fn wallet_poll_updates(
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<bool, WalletError> {
    let keys = state.signing_keys().map_err(WalletError::unavailable)?;
    let provider = wallet_manager()
        .provider_for(&keys, &app_data_dir(&app)?)
        .await?;
    let changed = provider.poll_updates().await?;
    let deposited = reconcile_agent_runtime_deposits(&app, &state, &provider).await?;
    Ok(changed || deposited)
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
    ZapAttemptStore::new(&app_data_dir(&app)?, &keys.public_key().to_hex())
        .pending_for_recipient(&recipient_pubkey, target_event_id.as_deref())
}

#[tauri::command]
pub async fn wallet_list_placeholder_message_zaps(
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<Vec<WalletPlaceholderMessageZap>, WalletError> {
    let keys = state.signing_keys().map_err(WalletError::unavailable)?;
    ZapAttemptStore::new(&app_data_dir(&app)?, &keys.public_key().to_hex()).settled_message_zaps()
}

fn validate_agent_runtime_quote(
    keys: &nostr::Keys,
    quote_event_json: &str,
) -> Result<(RuntimeQuote, WalletRecipientOffer), WalletError> {
    let quote_event = Event::from_json(quote_event_json)
        .map_err(|error| WalletError::new("runtime_quote_invalid", error.to_string()))?;
    quote_event
        .verify()
        .map_err(|error| WalletError::new("runtime_quote_invalid", error.to_string()))?;
    if quote_event.kind != Kind::Custom(KIND_AGENT_RUNTIME_RESPONSE as u16) {
        return Err(WalletError::new(
            "runtime_quote_invalid",
            "runtime quote must be a signed kind-24211 event",
        ));
    }
    let payer = keys.public_key().to_hex();
    let payer_tags = quote_event
        .tags
        .iter()
        .filter_map(|tag| {
            let parts = tag.as_slice();
            (parts.first().map(String::as_str) == Some("p"))
                .then(|| parts.get(1).cloned())
                .flatten()
        })
        .collect::<Vec<_>>();
    if payer_tags.as_slice() != [payer.as_str()] {
        return Err(WalletError::new(
            "runtime_quote_invalid",
            "runtime quote must address exactly the active payer",
        ));
    }
    let encryption_tags = quote_event
        .tags
        .iter()
        .filter(|tag| tag.as_slice().first().map(String::as_str) == Some("encryption"))
        .collect::<Vec<_>>();
    if encryption_tags.len() != 1
        || encryption_tags[0].as_slice().get(1).map(String::as_str) != Some("nip44_v2")
    {
        return Err(WalletError::new(
            "runtime_quote_invalid",
            "runtime quote must declare NIP-44 v2 encryption",
        ));
    }
    let plaintext = nip44::decrypt(keys.secret_key(), &quote_event.pubkey, &quote_event.content)
        .map_err(|error| WalletError::new("runtime_quote_invalid", error.to_string()))?;
    let response: RuntimeReservationResponse = serde_json::from_str(&plaintext)
        .map_err(|error| WalletError::new("runtime_quote_invalid", error.to_string()))?;
    response
        .validate()
        .map_err(|error| WalletError::new("runtime_quote_invalid", error.to_string()))?;
    let RuntimeReservationResponse::PaymentRequired { quote } = response else {
        return Err(WalletError::new(
            "runtime_quote_invalid",
            "runtime zap requires a payment-required response",
        ));
    };
    if quote.agent_pubkey != quote_event.pubkey.to_hex() || quote.payer_pubkey != payer {
        return Err(WalletError::new(
            "runtime_quote_invalid",
            "runtime quote parties do not match its signature and recipient",
        ));
    }
    let expirations = quote_event
        .tags
        .iter()
        .filter_map(|tag| {
            let parts = tag.as_slice();
            (parts.first().map(String::as_str) == Some("expiration"))
                .then(|| parts.get(1).cloned())
                .flatten()
        })
        .collect::<Vec<_>>();
    if expirations.len() != 1 || expirations[0].parse::<u64>().ok() != Some(quote.expires_at) {
        return Err(WalletError::new(
            "runtime_quote_invalid",
            "runtime quote expiration tag does not match its signed terms",
        ));
    }
    if quote_event.created_at.as_secs() > quote.expires_at
        || quote.expires_at > quote_event.created_at.as_secs().saturating_add(5 * 60)
    {
        return Err(WalletError::new(
            "runtime_quote_invalid",
            "runtime quote has an invalid validity interval",
        ));
    }
    let offer_json = serde_json::to_string(&quote.offer_event)
        .map_err(|error| WalletError::new("runtime_quote_invalid", error.to_string()))?;
    let offer_event = Event::from_json(&offer_json)
        .map_err(|error| WalletError::new("runtime_quote_invalid", error.to_string()))?;
    let recipient = recipient_offer(&offer_event, &quote.agent_pubkey)?;
    Ok((quote, recipient))
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
    let (quote, recipient) = validate_agent_runtime_quote(&keys, &request.quote_event_json)?;
    let payment_request = WalletProfileZapRequest {
        recipient_pubkey: quote.agent_pubkey.clone(),
        amount: quote.amount_sats,
        comment: None,
        idempotency_key: request.idempotency_key,
        target_event_id: None,
        target_event_kind: None,
    };
    wallet_send_zap_attempt(
        app,
        state,
        payment_request,
        Some((quote, recipient, request.quote_event_json)),
    )
    .await
}

async fn wallet_send_zap_attempt(
    app: AppHandle,
    state: State<'_, AppState>,
    request: WalletProfileZapRequest,
    prepared_runtime: Option<(RuntimeQuote, WalletRecipientOffer, String)>,
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
    let comment = request
        .comment
        .as_deref()
        .map(str::trim)
        .filter(|comment| !comment.is_empty())
        .map(str::to_string);
    let quote_json = prepared_runtime.as_ref().map(|(_, _, json)| json.clone());
    let runtime_channel = prepared_runtime
        .as_ref()
        .map(|(quote, _, _)| quote.channel_id.clone());
    let mut attempt = match store.load(&request.idempotency_key)? {
        Some(attempt)
            if attempt.recipient_pubkey == request.recipient_pubkey
                && attempt.amount == request.amount
                && attempt.comment == comment
                && attempt.target_event_id == request.target_event_id
                && attempt.target_event_kind == request.target_event_kind
                && attempt.runtime_quote_event_json == quote_json =>
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
            let mut attempt = if let Some((quote, recipient, quote_json)) = prepared_runtime {
                if now_ms() / 1_000 > quote.expires_at {
                    return Err(WalletError::new(
                        "runtime_quote_expired",
                        "runtime quote has expired; request a new reservation",
                    ));
                }
                ZapAttempt::prepare_agent_runtime(
                    request.idempotency_key,
                    recipient,
                    request.amount,
                    quote_json,
                    &keys,
                )?
            } else {
                let relay = relay_api_base_url_with_override(&state);
                let recipient =
                    resolve_recipient_offer(&state, &keys, &relay, &request.recipient_pubkey)
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
            store.save_prepared(&mut attempt)?;
            attempt
        }
    };

    match attempt.state {
        ZapAttemptState::PaidWithoutProof => {
            store.record_terminal_reuse(&attempt);
            if !attempt.proof_published {
                let channel = match runtime_channel.clone() {
                    Some(channel) => Some(channel),
                    None => zap_target_channel_id(&state, &keys, &attempt).await?,
                };
                let event =
                    store.prepare_placeholder_proof(&mut attempt, &keys, channel.as_deref())?;
                let relay = relay_api_base_url_with_override(&state);
                submit_signed_event_at_with_keys(&event, &state, &relay, &keys)
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

    let personal_note = if attempt.runtime_quote_event_json.is_some() {
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

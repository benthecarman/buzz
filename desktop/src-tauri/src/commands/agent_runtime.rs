use buzz_core_pkg::{
    agent_runtime_payment::{
        DepositRecord, ReservationRecord, RuntimeDeposit, RuntimeLedger, RuntimeReservation,
        RuntimeReservationRequest, RuntimeReservationResponse, RuntimeSettlement, SettlementRecord,
        VERSION,
    },
    kind::{
        KIND_AGENT_RUNTIME_DEPOSIT, KIND_AGENT_RUNTIME_REQUEST, KIND_AGENT_RUNTIME_RESERVATION,
        KIND_AGENT_RUNTIME_RESPONSE, KIND_AGENT_RUNTIME_SETTLEMENT,
    },
};
use nostr::{
    nips::nip44::{self, Version},
    EventBuilder, JsonUtil, Kind, PublicKey, Tag,
};
use serde::{Deserialize, Serialize};
use tauri::State;

use crate::{
    app_state::AppState,
    managed_agents::RelayAgentInfo,
    nostr_convert,
    relay::{
        query_relay_at_with_keys, relay_api_base_url_with_override,
        submit_signed_event_at_with_keys,
    },
};

const REQUEST_TTL_SECS: u64 = 5 * 60;
const RESPONSE_WAIT_ATTEMPTS: usize = 100;
const LEDGER_PAGE_SIZE: usize = 250;

pub async fn list_runtime_priced_relay_agents(
    state: State<'_, AppState>,
) -> Result<Vec<RelayAgentInfo>, String> {
    let keys = state.signing_keys()?;
    let relay = relay_api_base_url_with_override(&state);
    let events = query_all_runtime_events(
        &state,
        &relay,
        &keys,
        serde_json::json!({
            "kinds": [
                buzz_core_pkg::kind::KIND_AGENT_PROFILE,
                buzz_core_pkg::kind::KIND_AGENT_RUNTIME_PRICING
            ],
        }),
    )
    .await?;
    let profiles = events
        .iter()
        .filter(|event| event.kind.as_u16() as u32 == buzz_core_pkg::kind::KIND_AGENT_PROFILE)
        .cloned()
        .collect::<Vec<_>>();
    let mut prices = std::collections::HashMap::<String, &nostr::Event>::new();
    for event in events.iter().filter(|event| {
        event.kind.as_u16() as u32 == buzz_core_pkg::kind::KIND_AGENT_RUNTIME_PRICING
            && event.verify().is_ok()
    }) {
        let pubkey = event.pubkey.to_hex();
        if prices
            .get(&pubkey)
            .is_none_or(|current| (event.created_at, event.id) > (current.created_at, current.id))
        {
            prices.insert(pubkey, event);
        }
    }
    let value = nostr_convert::agents_from_events(&profiles);
    let mut agents: Vec<RelayAgentInfo> = serde_json::from_value(
        value
            .get("agents")
            .cloned()
            .unwrap_or_else(|| serde_json::json!([])),
    )
    .map_err(|error| format!("agent parse failed: {error}"))?;
    let agent_authors = agents
        .iter()
        .map(|agent| agent.pubkey.clone())
        .collect::<Vec<_>>();
    let owner_profiles = if agent_authors.is_empty() {
        Vec::new()
    } else {
        query_all_runtime_events(
            &state,
            &relay,
            &keys,
            serde_json::json!({ "kinds": [0], "authors": agent_authors }),
        )
        .await?
    };
    let mut owners =
        std::collections::HashMap::<String, (nostr::Timestamp, nostr::EventId, String)>::new();
    for event in owner_profiles {
        if event.verify().is_err() {
            continue;
        }
        let Some(owner) = nostr_convert::profile_valid_oa_owner_pubkey(&event) else {
            continue;
        };
        let agent_pubkey = event.pubkey.to_hex();
        let candidate = (event.created_at, event.id, owner);
        if owners
            .get(&agent_pubkey)
            .is_none_or(|current| (candidate.0, candidate.1) > (current.0, current.1))
        {
            owners.insert(agent_pubkey, candidate);
        }
    }
    for agent in &mut agents {
        agent.owner_pubkey = owners.get(&agent.pubkey).map(|(_, _, owner)| owner.clone());
        agent.price_per_minute_sats = prices
            .get(&agent.pubkey)
            .and_then(|event| {
                serde_json::from_str::<buzz_core_pkg::agent_runtime_payment::RuntimePricing>(
                    &event.content,
                )
                .ok()
            })
            .filter(|pricing| pricing.validate().is_ok() && pricing.enabled)
            .and_then(|pricing| pricing.rate_sats_per_minute);
    }
    Ok(agents)
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentRuntimeReservationInput {
    pub agent_pubkey: String,
    pub channel_id: String,
    pub cap_minutes: u16,
    #[serde(default)]
    pub request_id: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentRuntimeReservationResult {
    pub request_id: String,
    pub request_event_id: String,
    pub response_event_json: String,
    pub response: RuntimeReservationResponse,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentRuntimeBalanceInput {
    pub agent_pubkey: String,
    pub channel_id: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentRuntimeBalanceResult {
    pub available_ms: u64,
    pub credited_ms: u64,
    pub used_ms: u64,
}

fn exactly_one_tag<'a>(event: &'a nostr::Event, name: &str) -> Result<&'a str, String> {
    let values = event
        .tags
        .iter()
        .filter_map(|tag| {
            let parts = tag.as_slice();
            (parts.len() == 2 && parts[0].as_str() == name).then(|| parts[1].as_str())
        })
        .collect::<Vec<_>>();
    match values.as_slice() {
        [value] => Ok(value),
        _ => Err(format!("expected exactly one {name} tag")),
    }
}

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn validate_agent_response(
    keys: &nostr::Keys,
    agent: &PublicKey,
    event: &nostr::Event,
    request: &RuntimeReservationRequest,
    response: &RuntimeReservationResponse,
) -> Result<(), String> {
    let now = now_secs();
    let response_expiry = exactly_one_tag(event, "expiration")?
        .parse::<u64>()
        .map_err(|_| "runtime response has an invalid expiration".to_string())?;
    if event.pubkey != *agent
        || event.kind != Kind::Custom(KIND_AGENT_RUNTIME_RESPONSE as u16)
        || exactly_one_tag(event, "p")? != keys.public_key().to_hex()
        || exactly_one_tag(event, "encryption")? != "nip44_v2"
        || response_expiry < now
        || response_expiry > now.saturating_add(REQUEST_TTL_SECS)
    {
        return Err("runtime response routing or expiration is invalid".into());
    }
    match response {
        RuntimeReservationResponse::Reserved {
            request_id,
            reservation_event,
            ..
        } => {
            let reservation_event = nostr::Event::from_json(reservation_event.to_string())
                .map_err(|error| format!("decode signed runtime reservation: {error}"))?;
            reservation_event
                .verify()
                .map_err(|error| format!("verify signed runtime reservation: {error}"))?;
            let reservation_expiry = exactly_one_tag(&reservation_event, "expiration")?
                .parse::<u64>()
                .map_err(|_| "runtime reservation has an invalid expiration".to_string())?;
            let plaintext = nip44::decrypt(keys.secret_key(), agent, &reservation_event.content)
                .map_err(|error| format!("decrypt signed runtime reservation: {error}"))?;
            let reservation: RuntimeReservation = serde_json::from_str(&plaintext)
                .map_err(|error| format!("decode runtime reservation terms: {error}"))?;
            reservation.validate().map_err(|error| error.to_string())?;
            if reservation_event.pubkey != *agent
                || reservation_event.kind != Kind::Custom(KIND_AGENT_RUNTIME_RESERVATION as u16)
                || exactly_one_tag(&reservation_event, "p")? != keys.public_key().to_hex()
                || exactly_one_tag(&reservation_event, "h")? != request.channel_id
                || exactly_one_tag(&reservation_event, "encryption")? != "nip44_v2"
                || request_id != &request.request_id
                || reservation.request_id != request.request_id
                || reservation.cap_ms != u64::from(request.cap_minutes) * 60_000
                || reservation.must_start_by != reservation_expiry
                || reservation_event.created_at.as_secs() > reservation_expiry
                || reservation_expiry
                    > reservation_event
                        .created_at
                        .as_secs()
                        .saturating_add(REQUEST_TTL_SECS)
                || reservation_expiry < now
            {
                return Err("signed runtime reservation does not match the request".into());
            }
        }
        RuntimeReservationResponse::PaymentRequired { quote } => {
            if quote.request_id != request.request_id
                || quote.agent_pubkey != agent.to_hex()
                || quote.payer_pubkey != keys.public_key().to_hex()
                || quote.channel_id != request.channel_id
                || quote.cap_minutes != request.cap_minutes
                || quote.pack_minutes != request.cap_minutes
                || quote.expires_at != response_expiry
                || event.created_at.as_secs() > quote.expires_at
            {
                return Err("signed runtime quote does not match the request".into());
            }
        }
        RuntimeReservationResponse::Unavailable { request_id, .. } => {
            if request_id != &request.request_id {
                return Err("runtime unavailability response is not correlated".into());
            }
        }
    }
    Ok(())
}

async fn query_all_runtime_events(
    state: &AppState,
    relay: &str,
    keys: &nostr::Keys,
    base_filter: serde_json::Value,
) -> Result<Vec<nostr::Event>, String> {
    let mut until = None;
    let mut seen = std::collections::HashSet::new();
    let mut events = Vec::new();
    loop {
        let mut filter = base_filter.clone();
        let object = filter
            .as_object_mut()
            .ok_or_else(|| "runtime ledger filter is not an object".to_string())?;
        object.insert("limit".into(), serde_json::Value::from(LEDGER_PAGE_SIZE));
        if let Some(cursor) = until {
            object.insert("until".into(), serde_json::Value::from(cursor));
        }
        let page =
            query_relay_at_with_keys(state, relay, std::slice::from_ref(&filter), keys, None)
                .await?;
        if page.is_empty() {
            break;
        }
        let page_len = page.len();
        let oldest = page
            .iter()
            .map(|event| event.created_at.as_secs())
            .min()
            .ok_or_else(|| "runtime ledger page has no timestamp".to_string())?;
        let mut inserted = 0usize;
        for event in page {
            if seen.insert(event.id) {
                inserted += 1;
                events.push(event);
            }
        }
        if page_len < LEDGER_PAGE_SIZE {
            break;
        }
        if until == Some(oldest) && inserted == 0 {
            return Err(
                "runtime ledger pagination cannot advance across one dense timestamp".into(),
            );
        }
        until = Some(oldest);
    }
    Ok(events)
}

#[tauri::command]
pub async fn agent_runtime_request_reservation(
    state: State<'_, AppState>,
    input: AgentRuntimeReservationInput,
) -> Result<AgentRuntimeReservationResult, String> {
    let keys = state.signing_keys()?;
    let agent = PublicKey::from_hex(&input.agent_pubkey)
        .map_err(|error| format!("invalid agent pubkey: {error}"))?;
    let request_id = input
        .request_id
        .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
    let request = RuntimeReservationRequest {
        version: VERSION,
        request_id: request_id.clone(),
        channel_id: input.channel_id,
        cap_minutes: input.cap_minutes,
    };
    request.validate().map_err(|error| error.to_string())?;
    let plaintext = serde_json::to_string(&request).map_err(|error| error.to_string())?;
    let ciphertext = nip44::encrypt(keys.secret_key(), &agent, plaintext, Version::V2)
        .map_err(|error| format!("encrypt runtime reservation request: {error}"))?;
    let expires_at = now_secs().saturating_add(REQUEST_TTL_SECS);
    let request_event =
        EventBuilder::new(Kind::Custom(KIND_AGENT_RUNTIME_REQUEST as u16), ciphertext)
            .tags([
                Tag::parse(["p", input.agent_pubkey.as_str()])
                    .map_err(|error| error.to_string())?,
                Tag::parse(["expiration", expires_at.to_string().as_str()])
                    .map_err(|error| error.to_string())?,
                Tag::parse(["encryption", "nip44_v2"]).map_err(|error| error.to_string())?,
            ])
            .sign_with_keys(&keys)
            .map_err(|error| format!("sign runtime reservation request: {error}"))?;
    let relay = relay_api_base_url_with_override(&state);
    submit_signed_event_at_with_keys(&request_event, &state, &relay, &keys)
        .await
        .map_err(|error| format!("publish runtime reservation request: {error}"))?;

    let payer_hex = keys.public_key().to_hex();
    let since = now_secs().saturating_sub(2);
    let filter = serde_json::json!({
        "kinds": [KIND_AGENT_RUNTIME_RESPONSE],
        "authors": [input.agent_pubkey],
        "#p": [payer_hex],
        "since": since,
        "limit": 100
    });
    for _ in 0..RESPONSE_WAIT_ATTEMPTS {
        let mut events =
            query_relay_at_with_keys(&state, &relay, std::slice::from_ref(&filter), &keys, None)
                .await?;
        events.sort_by_key(|event| (event.created_at, event.id));
        for event in events.into_iter().rev() {
            if event.verify().is_err() || event.pubkey != agent {
                continue;
            }
            let plaintext = match nip44::decrypt(keys.secret_key(), &agent, &event.content) {
                Ok(value) => value,
                Err(_) => continue,
            };
            let response = match serde_json::from_str::<RuntimeReservationResponse>(&plaintext) {
                Ok(value) if value.validate().is_ok() => value,
                _ => continue,
            };
            let response_request_id = match &response {
                RuntimeReservationResponse::Reserved { request_id, .. }
                | RuntimeReservationResponse::Unavailable { request_id, .. } => request_id,
                RuntimeReservationResponse::PaymentRequired { quote } => &quote.request_id,
            };
            if response_request_id == &request_id {
                validate_agent_response(&keys, &agent, &event, &request, &response)?;
                return Ok(AgentRuntimeReservationResult {
                    request_id,
                    request_event_id: request_event.id.to_hex(),
                    response_event_json: event.as_json(),
                    response,
                });
            }
        }
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    }
    Err("timed out waiting for the agent runtime reservation response".into())
}

#[tauri::command]
pub async fn agent_runtime_get_balance(
    state: State<'_, AppState>,
    input: AgentRuntimeBalanceInput,
) -> Result<AgentRuntimeBalanceResult, String> {
    let keys = state.signing_keys()?;
    let agent = PublicKey::from_hex(&input.agent_pubkey)
        .map_err(|error| format!("invalid agent pubkey: {error}"))?;
    let relay = relay_api_base_url_with_override(&state);
    let payer_hex = keys.public_key().to_hex();
    let filter = serde_json::json!({
        "kinds": [
            KIND_AGENT_RUNTIME_DEPOSIT,
            KIND_AGENT_RUNTIME_RESERVATION,
            KIND_AGENT_RUNTIME_SETTLEMENT
        ],
        "authors": [input.agent_pubkey],
        "#p": [payer_hex],
        "#h": [input.channel_id]
    });
    let mut events = query_all_runtime_events(&state, &relay, &keys, filter).await?;
    events.sort_by_key(|event| {
        let phase = match u32::from(event.kind.as_u16()) {
            KIND_AGENT_RUNTIME_DEPOSIT => 0u8,
            KIND_AGENT_RUNTIME_RESERVATION => 1,
            KIND_AGENT_RUNTIME_SETTLEMENT => 2,
            _ => 3,
        };
        (event.created_at, phase, event.id)
    });
    let mut ledger = RuntimeLedger::default();
    for event in events {
        event
            .verify()
            .map_err(|error| format!("verify runtime ledger event: {error}"))?;
        if event.pubkey != agent
            || exactly_one_tag(&event, "p")? != keys.public_key().to_hex()
            || exactly_one_tag(&event, "h")? != input.channel_id
        {
            return Err("runtime ledger routing does not match the requested scope".into());
        }
        match u32::from(event.kind.as_u16()) {
            KIND_AGENT_RUNTIME_DEPOSIT => {
                let deposit: RuntimeDeposit = serde_json::from_str(&event.content)
                    .map_err(|error| format!("decode runtime deposit: {error}"))?;
                deposit.validate().map_err(|error| error.to_string())?;
                nostr::EventId::from_hex(exactly_one_tag(&event, "quote")?)
                    .map_err(|error| format!("invalid runtime deposit quote: {error}"))?;
                nostr::EventId::from_hex(exactly_one_tag(&event, "zap")?)
                    .map_err(|error| format!("invalid runtime deposit zap: {error}"))?;
                ledger
                    .apply_deposit(DepositRecord {
                        payment_id: exactly_one_tag(&event, "zap_intent")?.to_string(),
                        credit_ms: deposit.credit_ms,
                    })
                    .map_err(|error| error.to_string())?;
            }
            KIND_AGENT_RUNTIME_RESERVATION => {
                let plaintext = nip44::decrypt(keys.secret_key(), &agent, &event.content)
                    .map_err(|error| format!("decrypt runtime reservation: {error}"))?;
                let reservation: RuntimeReservation = serde_json::from_str(&plaintext)
                    .map_err(|error| format!("decode runtime reservation: {error}"))?;
                reservation.validate().map_err(|error| error.to_string())?;
                let expiration = exactly_one_tag(&event, "expiration")?
                    .parse::<u64>()
                    .map_err(|_| "runtime reservation expiration is invalid".to_string())?;
                if exactly_one_tag(&event, "encryption")? != "nip44_v2"
                    || expiration != reservation.must_start_by
                    || event.created_at.as_secs() > expiration
                    || expiration > event.created_at.as_secs().saturating_add(REQUEST_TTL_SECS)
                {
                    return Err("runtime reservation validity tags are invalid".into());
                }
                ledger
                    .apply_reservation(ReservationRecord {
                        reservation_id: event.id.to_hex(),
                        cap_ms: reservation.cap_ms,
                    })
                    .map_err(|error| error.to_string())?;
            }
            KIND_AGENT_RUNTIME_SETTLEMENT => {
                let plaintext = nip44::decrypt(keys.secret_key(), &agent, &event.content)
                    .map_err(|error| format!("decrypt runtime settlement: {error}"))?;
                let settlement: RuntimeSettlement = serde_json::from_str(&plaintext)
                    .map_err(|error| format!("decode runtime settlement: {error}"))?;
                settlement.validate().map_err(|error| error.to_string())?;
                if exactly_one_tag(&event, "encryption")? != "nip44_v2"
                    || exactly_one_tag(&event, "e")? != settlement.reservation_id
                    || ledger.reservation_cap_ms(&settlement.reservation_id)
                        != Some(settlement.cap_ms)
                {
                    return Err("runtime settlement does not match its reservation".into());
                }
                ledger
                    .apply_settlement(SettlementRecord {
                        reservation_id: settlement.reservation_id,
                        used_ms: settlement.used_ms,
                    })
                    .map_err(|error| error.to_string())?;
            }
            _ => return Err("unexpected runtime ledger event kind".into()),
        }
    }
    Ok(AgentRuntimeBalanceResult {
        available_ms: ledger.available_ms().map_err(|error| error.to_string())?,
        credited_ms: ledger.credited_ms(),
        used_ms: ledger.used_ms(),
    })
}

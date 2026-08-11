use buzz_core_pkg::{
    agent_runtime_payment::{
        DepositRecord, ReservationRecord, RuntimeDeposit, RuntimeLedger, RuntimePricing,
        RuntimeReservation, RuntimeSettlement, SettlementRecord, RESERVATION_VALIDITY_SECS,
    },
    kind::{
        KIND_AGENT_RUNTIME_DEPOSIT, KIND_AGENT_RUNTIME_PRICING, KIND_AGENT_RUNTIME_RESERVATION,
        KIND_AGENT_RUNTIME_SETTLEMENT,
    },
};
use nostr::{nips::nip44, JsonUtil, PublicKey};
use serde::{Deserialize, Serialize};
use tauri::State;

use crate::{
    app_state::AppState,
    managed_agents::RelayAgentInfo,
    nostr_convert,
    relay::{query_relay_at_with_keys, relay_api_base_url_with_override},
};

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
pub struct AgentRuntimeStatusInput {
    pub agent_pubkey: String,
    pub channel_id: String,
}

/// One open, claimable reservation read out of the payer's own ledger.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentRuntimeOpenReservation {
    /// Exact signed kind-44211 event, attached to the instruction verbatim.
    pub reservation_event_json: String,
    pub reservation_event_id: String,
    pub cap_ms: u64,
    pub must_start_by: u64,
}

/// The Agent's published terms, pinned for a purchase.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentRuntimePricingTerms {
    /// Exact signed kind-10101 event the purchase will pay against.
    pub pricing_event_json: String,
    pub rate_sats_per_minute: u64,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentRuntimeStatusResult {
    pub available_ms: u64,
    pub credited_ms: u64,
    pub used_ms: u64,
    /// Present when a claimable lock is waiting — invoke immediately.
    pub open_reservation: Option<AgentRuntimeOpenReservation>,
    /// Present when the Agent currently advertises an enabled rate.
    pub pricing: Option<AgentRuntimePricingTerms>,
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

/// Everything the checkout needs about one (agent, channel) scope, read from
/// durable state alone: retained balance, the open reservation if the Agent
/// has minted one, and the published terms a new purchase would pay against.
/// There is nothing to ask the Agent — this is the whole payer protocol.
#[tauri::command]
pub async fn agent_runtime_get_status(
    state: State<'_, AppState>,
    input: AgentRuntimeStatusInput,
) -> Result<AgentRuntimeStatusResult, String> {
    let keys = state.signing_keys()?;
    let agent = PublicKey::from_hex(&input.agent_pubkey)
        .map_err(|error| format!("invalid agent pubkey: {error}"))?;
    let relay = relay_api_base_url_with_override(&state);
    let payer_hex = keys.public_key().to_hex();
    // No `#h` here, deliberately: ledger kinds are stored channel-less, and
    // the relay's `#h` handling scopes to the stored channel column (NULL for
    // these kinds), so an `#h` filter returns nothing. The signed `h` tag is
    // checked per event below instead.
    let filter = serde_json::json!({
        "kinds": [
            KIND_AGENT_RUNTIME_DEPOSIT,
            KIND_AGENT_RUNTIME_RESERVATION,
            KIND_AGENT_RUNTIME_SETTLEMENT
        ],
        "authors": [input.agent_pubkey],
        "#p": [payer_hex]
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
    let mut reservations: Vec<(nostr::Event, RuntimeReservation)> = Vec::new();
    for event in events {
        event
            .verify()
            .map_err(|error| format!("verify runtime ledger event: {error}"))?;
        if event.pubkey != agent || exactly_one_tag(&event, "p")? != keys.public_key().to_hex() {
            return Err("runtime ledger routing does not match the requested scope".into());
        }
        // The query is payer-wide; this status is per channel.
        if exactly_one_tag(&event, "h")? != input.channel_id {
            continue;
        }
        match u32::from(event.kind.as_u16()) {
            KIND_AGENT_RUNTIME_DEPOSIT => {
                let deposit: RuntimeDeposit = serde_json::from_str(&event.content)
                    .map_err(|error| format!("decode runtime deposit: {error}"))?;
                deposit.validate().map_err(|error| error.to_string())?;
                nostr::EventId::from_hex(exactly_one_tag(&event, "pricing")?)
                    .map_err(|error| format!("invalid runtime deposit pricing: {error}"))?;
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
                    || expiration
                        > event
                            .created_at
                            .as_secs()
                            .saturating_add(RESERVATION_VALIDITY_SECS)
                {
                    return Err("runtime reservation validity tags are invalid".into());
                }
                ledger
                    .apply_reservation(ReservationRecord {
                        reservation_id: event.id.to_hex(),
                        cap_ms: reservation.cap_ms,
                    })
                    .map_err(|error| error.to_string())?;
                reservations.push((event, reservation));
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
    // The newest still-claimable lock wins; anything expired is the Agent's
    // sweep to clean up, not ours to offer.
    let now = now_secs();
    let open_reservation = reservations
        .into_iter()
        .filter(|(event, reservation)| {
            ledger.reservation_is_open(&event.id.to_hex()) && reservation.must_start_by > now
        })
        .max_by_key(|(event, _)| (event.created_at, event.id))
        .map(|(event, reservation)| AgentRuntimeOpenReservation {
            reservation_event_id: event.id.to_hex(),
            reservation_event_json: event.as_json(),
            cap_ms: reservation.cap_ms,
            must_start_by: reservation.must_start_by,
        });
    let pricing = fetch_pricing_terms(&state, &relay, &keys, &agent).await?;
    Ok(AgentRuntimeStatusResult {
        available_ms: ledger.available_ms().map_err(|error| error.to_string())?,
        credited_ms: ledger.credited_ms(),
        used_ms: ledger.used_ms(),
        open_reservation,
        pricing,
    })
}

/// The Agent's latest enabled pricing, or None when it does not charge.
async fn fetch_pricing_terms(
    state: &State<'_, AppState>,
    relay: &str,
    keys: &nostr::Keys,
    agent: &PublicKey,
) -> Result<Option<AgentRuntimePricingTerms>, String> {
    let filter = serde_json::json!({
        "kinds": [KIND_AGENT_RUNTIME_PRICING],
        "authors": [agent.to_hex()],
        "limit": 1,
    });
    let events =
        query_relay_at_with_keys(state, relay, std::slice::from_ref(&filter), keys, None).await?;
    let Some(event) = events
        .into_iter()
        .filter(|event| event.verify().is_ok() && event.pubkey == *agent)
        .max_by_key(|event| (event.created_at, event.id))
    else {
        return Ok(None);
    };
    let Ok(pricing) = serde_json::from_str::<RuntimePricing>(&event.content) else {
        return Ok(None);
    };
    if pricing.validate().is_err() || !pricing.enabled {
        return Ok(None);
    }
    let Some(rate) = pricing.rate_sats_per_minute else {
        return Ok(None);
    };
    Ok(Some(AgentRuntimePricingTerms {
        pricing_event_json: event.as_json(),
        rate_sats_per_minute: rate,
    }))
}

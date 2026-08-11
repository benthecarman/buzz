use buzz_core_pkg::{
    agent_runtime_payment::RuntimePricing,
    kind::{KIND_AGENT_RUNTIME_PRICING, KIND_BOLT12_ZAP},
};
use nostr::{JsonUtil, PublicKey};
use serde::{Deserialize, Serialize};
use tauri::State;

use crate::{
    app_state::AppState,
    managed_agents::RelayAgentInfo,
    nostr_convert,
    relay::{query_relay_at_with_keys, relay_api_base_url_with_override},
    wallet::zap::parse_tagged_zap_event,
};

pub async fn list_runtime_priced_relay_agents(
    state: State<'_, AppState>,
) -> Result<Vec<RelayAgentInfo>, String> {
    let keys = state.signing_keys()?;
    let relay = relay_api_base_url_with_override(&state);
    let events = query_relay_at_with_keys(
        &state,
        &relay,
        &[serde_json::json!({
            "kinds": [
                buzz_core_pkg::kind::KIND_AGENT_PROFILE,
                KIND_AGENT_RUNTIME_PRICING
            ],
        })],
        &keys,
        None,
    )
    .await?;
    let profiles = events
        .iter()
        .filter(|event| u32::from(event.kind.as_u16()) == buzz_core_pkg::kind::KIND_AGENT_PROFILE)
        .cloned()
        .collect::<Vec<_>>();
    let mut prices = std::collections::HashMap::<String, &nostr::Event>::new();
    for event in events.iter().filter(|event| {
        u32::from(event.kind.as_u16()) == KIND_AGENT_RUNTIME_PRICING && event.verify().is_ok()
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
        query_relay_at_with_keys(
            &state,
            &relay,
            &[serde_json::json!({ "kinds": [0], "authors": agent_authors })],
            &keys,
            None,
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
            .and_then(|event| serde_json::from_str::<RuntimePricing>(&event.content).ok())
            .filter(|pricing| pricing.validate().is_ok() && pricing.enabled)
            .and_then(|pricing| pricing.price_sats);
    }
    Ok(agents)
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentRuntimeStatusInput {
    pub agent_pubkey: String,
    pub channel_id: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentRuntimeAccessZap {
    pub zap_event_id: String,
    pub created_at: u64,
    pub valid_until: u64,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentRuntimePricingTerms {
    pub pricing_event_json: String,
    pub price_sats: u64,
    pub invocation_window_seconds: u64,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentRuntimeStatusResult {
    pub access_zap: Option<AgentRuntimeAccessZap>,
    pub pricing: Option<AgentRuntimePricingTerms>,
}

#[tauri::command]
pub async fn agent_runtime_get_status(
    state: State<'_, AppState>,
    input: AgentRuntimeStatusInput,
) -> Result<AgentRuntimeStatusResult, String> {
    let keys = state.signing_keys()?;
    let agent = PublicKey::from_hex(&input.agent_pubkey)
        .map_err(|error| format!("invalid agent pubkey: {error}"))?;
    let relay = relay_api_base_url_with_override(&state);
    let Some((pricing_event, pricing)) = fetch_pricing_terms(&state, &relay, &keys, &agent).await?
    else {
        return Ok(AgentRuntimeStatusResult {
            access_zap: None,
            pricing: None,
        });
    };
    let price_sats = pricing
        .price_sats
        .ok_or_else(|| "enabled pricing has no price".to_string())?;
    let invocation_window_seconds = pricing
        .invocation_window_seconds
        .ok_or_else(|| "enabled pricing has no invocation window".to_string())?;
    let terms = AgentRuntimePricingTerms {
        pricing_event_json: pricing_event.as_json(),
        price_sats,
        invocation_window_seconds,
    };
    let payer = keys.public_key().to_hex();
    let zap_events = query_relay_at_with_keys(
        &state,
        &relay,
        &[serde_json::json!({
            "kinds": [KIND_BOLT12_ZAP],
            "authors": [payer],
            "#p": [input.agent_pubkey],
            "#h": [input.channel_id],
            "limit": 100,
        })],
        &keys,
        None,
    )
    .await?;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let access_zap = zap_events
        .into_iter()
        .filter_map(|event| {
            let raw = serde_json::to_value(&event).ok()?;
            let zap = parse_tagged_zap_event(&raw).ok()?;
            let valid_until = event
                .created_at
                .as_secs()
                .checked_add(invocation_window_seconds)?;
            (zap.recipient_pubkey == input.agent_pubkey
                && zap.target_event_id.as_deref() == Some(pricing_event.id.to_hex().as_str())
                && zap.channel_id.as_deref() == Some(input.channel_id.as_str())
                && zap.amount == price_sats
                && now <= valid_until)
                .then_some(AgentRuntimeAccessZap {
                    zap_event_id: event.id.to_hex(),
                    created_at: event.created_at.as_secs(),
                    valid_until,
                })
        })
        .max_by_key(|zap| (zap.created_at, zap.zap_event_id.clone()));
    Ok(AgentRuntimeStatusResult {
        access_zap,
        pricing: Some(terms),
    })
}

async fn fetch_pricing_terms(
    state: &State<'_, AppState>,
    relay: &str,
    keys: &nostr::Keys,
    agent: &PublicKey,
) -> Result<Option<(nostr::Event, RuntimePricing)>, String> {
    let events = query_relay_at_with_keys(
        state,
        relay,
        &[serde_json::json!({
            "kinds": [KIND_AGENT_RUNTIME_PRICING],
            "authors": [agent.to_hex()],
            "limit": 1,
        })],
        keys,
        None,
    )
    .await?;
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
    Ok(Some((event, pricing)))
}

#[cfg(test)]
mod tests {
    use super::*;

    const CONTRACT: &str = include_str!("../../../fixtures/agent-runtime-contract.json");

    #[test]
    fn status_serialization_matches_the_desktop_contract_fixture() {
        let fixture: serde_json::Value = serde_json::from_str(CONTRACT).unwrap();
        let status = AgentRuntimeStatusResult {
            access_zap: Some(AgentRuntimeAccessZap {
                zap_event_id: "b".repeat(64),
                created_at: 4_102_444_500,
                valid_until: 4_102_444_800,
            }),
            pricing: Some(AgentRuntimePricingTerms {
                pricing_event_json: "{\"kind\":10101}".into(),
                price_sats: 255,
                invocation_window_seconds: 300,
            }),
        };
        assert_eq!(serde_json::to_value(&status).unwrap(), fixture["status"]);
    }

    #[test]
    fn fixture_invocation_builds_a_paid_message() {
        let fixture: serde_json::Value = serde_json::from_str(CONTRACT).unwrap();
        let runtime_tags: Vec<Vec<String>> =
            serde_json::from_value(fixture["expectedInvocation"]["runtimeTags"].clone()).unwrap();
        let builder = crate::events::build_message(
            uuid::Uuid::new_v4(),
            "run it",
            None,
            &[],
            &[],
            &[],
            &[],
            &[],
            &runtime_tags,
            "http://localhost:3000",
        )
        .expect("the invocation from the fixture must build");
        let event = builder.sign_with_keys(&nostr::Keys::generate()).unwrap();
        for tag in &runtime_tags {
            assert!(event.tags.iter().any(|item| item.as_slice() == tag));
        }
    }
}

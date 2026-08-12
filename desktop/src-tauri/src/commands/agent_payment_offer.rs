//! Payment-offer readiness for paid agent runtime.
//!
//! An agent may only advertise a per-minute rate once it has published the
//! kind:10058 BOLT12 offer a payer settles against. The harness enforces the
//! same precondition at startup (`validate_pricing_readiness` in `buzz-acp`)
//! and refuses to run without it, so an agent priced without one never starts
//! again.
//!
//! Missing offers used to be unrecoverable: only enabling the wallet published
//! them, and disabling the wallet is refused while any agent is priced. So the
//! rate is not merely gated here — the offer is published from the wallet the
//! owner already has, and only a genuinely absent wallet fails the edit.

use tauri::AppHandle;

use crate::app_state::AppState;

/// Ensure a rate about to be stored is one the agent could actually collect.
///
/// A no-op for every update that does not set a rate, so ordinary edits pay no
/// relay round-trip.
pub(super) async fn ensure_price_is_payable(
    app: &AppHandle,
    state: &AppState,
    input: &crate::managed_agents::UpdateManagedAgentRequest,
) -> Result<(), String> {
    if input.price_per_minute_sats.flatten().is_none() {
        return Ok(());
    }
    if agent_has_payment_offer(state, &input.pubkey).await? {
        return Ok(());
    }
    publish_agent_payment_offer(app, state, &input.pubkey).await
}

/// Publish this agent's offer from the owner's existing wallet.
///
/// The shared offer helper provisions current provider releases before it
/// creates the agent-scoped offer.
#[cfg(feature = "bitcoin")]
async fn publish_agent_payment_offer(
    app: &AppHandle,
    state: &AppState,
    agent_pubkey: &str,
) -> Result<(), String> {
    let agent_keys = agent_signing_keys(app, state, agent_pubkey)?;
    let relay_urls = vec![crate::relay::relay_ws_url_with_override(state)];
    match crate::commands::wallet::enabled::provision_new_managed_agent_offer(
        app,
        state,
        &agent_keys,
        relay_urls,
    )
    .await
    {
        Ok(warnings) => {
            for warning in warnings {
                tracing::warn!(agent_pubkey, warning, "agent payment offer warning");
            }
            Ok(())
        }
        Err(error) => {
            tracing::warn!(agent_pubkey, error, "publish agent payment offer");
            Err(MISSING_PAYMENT_OFFER_MESSAGE.to_string())
        }
    }
}

#[cfg(not(feature = "bitcoin"))]
async fn publish_agent_payment_offer(
    _app: &AppHandle,
    _state: &AppState,
    _agent_pubkey: &str,
) -> Result<(), String> {
    Err(MISSING_PAYMENT_OFFER_MESSAGE.to_string())
}

/// Load the agent's own signing keys, verifying they match its public key.
#[cfg(feature = "bitcoin")]
fn agent_signing_keys(
    app: &AppHandle,
    state: &AppState,
    agent_pubkey: &str,
) -> Result<nostr::Keys, String> {
    let records = {
        let _guard = state
            .managed_agents_store_lock
            .lock()
            .map_err(|error| error.to_string())?;
        crate::managed_agents::load_managed_agents(app)?
    };
    let record = records
        .iter()
        .find(|record| record.pubkey == agent_pubkey)
        .ok_or_else(|| format!("agent {agent_pubkey} not found"))?;
    let keys = nostr::Keys::parse(record.private_key_nsec.trim())
        .map_err(|error| format!("load agent signing key: {error}"))?;
    if keys.public_key().to_hex() != record.pubkey {
        return Err("agent signing key does not match its public key".to_string());
    }
    Ok(keys)
}

/// Whether this agent has published the BOLT12 offer that paid runtime needs.
async fn agent_has_payment_offer(state: &AppState, agent_pubkey: &str) -> Result<bool, String> {
    let keys = state.signing_keys()?;
    let relay = crate::relay::relay_api_base_url_with_override(state);
    let filter = serde_json::json!({
        "kinds": [buzz_core_pkg::kind::KIND_BOLT12_OFFER],
        "authors": [agent_pubkey],
        "limit": 8,
    });
    let events = crate::relay::query_relay_at_with_keys(
        state,
        &relay,
        std::slice::from_ref(&filter),
        &keys,
        None,
    )
    .await?;
    Ok(events.iter().any(event_advertises_offer))
}

/// Message shown when a rate is set on an agent nobody could pay.
const MISSING_PAYMENT_OFFER_MESSAGE: &str =
    "This agent has no Bitcoin payment offer, and one could not be published. \
     Enable the wallet in Settings, then set the rate.";

/// Whether a kind:10058 event carries a payable offer.
///
/// The signature is checked here rather than trusted from the query: an
/// unsigned or empty announcement would let a rate be set that no payer could
/// ever settle. Mirrors the harness's own `validate_first_offer`.
fn event_advertises_offer(event: &nostr::Event) -> bool {
    event.verify().is_ok()
        && event.tags.iter().any(|tag| {
            let parts = tag.as_slice();
            parts.len() >= 2 && parts[0] == "offer" && !parts[1].trim().is_empty()
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A rate is only settable on an agent someone can actually pay. Accepting
    /// an announcement without a payable offer would save the price and then
    /// strand the agent, which refuses to start without one.
    #[test]
    fn payment_offer_detection_requires_a_signed_non_empty_offer() {
        use nostr::{EventBuilder, Keys, Kind, Tag};

        let agent = Keys::generate();
        let offer_kind = Kind::Custom(buzz_core_pkg::kind::KIND_BOLT12_OFFER as u16);

        let published = EventBuilder::new(offer_kind, "")
            .tag(Tag::parse(["offer", "lno1qcp4256ypq"]).unwrap())
            .sign_with_keys(&agent)
            .unwrap();
        assert!(event_advertises_offer(&published));

        let withdrawn = EventBuilder::new(offer_kind, "")
            .sign_with_keys(&agent)
            .unwrap();
        assert!(
            !event_advertises_offer(&withdrawn),
            "an offer withdrawal must not count as payable"
        );

        let blank = EventBuilder::new(offer_kind, "")
            .tag(Tag::parse(["offer", "   "]).unwrap())
            .sign_with_keys(&agent)
            .unwrap();
        assert!(
            !event_advertises_offer(&blank),
            "a blank offer value must not count as payable"
        );
    }
}

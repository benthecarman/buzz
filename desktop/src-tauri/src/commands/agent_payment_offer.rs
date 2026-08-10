//! Payment-offer readiness for paid agent runtime.
//!
//! An agent may only advertise a per-minute rate once it has published the
//! kind:10058 BOLT12 offer a payer settles against. The harness enforces the
//! same precondition at startup (`validate_pricing_readiness` in `buzz-acp`)
//! and refuses to run without it, so checking before the rate is stored turns
//! a bricked agent into a rejected edit.

use crate::app_state::AppState;

/// Reject an update that turns on a rate the agent could never collect.
///
/// A no-op for every update that does not set a rate, so ordinary edits pay
/// no relay round-trip.
pub(super) async fn ensure_price_is_payable(
    state: &AppState,
    input: &crate::managed_agents::UpdateManagedAgentRequest,
) -> Result<(), String> {
    if input.price_per_minute_sats.flatten().is_none() {
        return Ok(());
    }
    if agent_has_payment_offer(state, &input.pubkey).await? {
        return Ok(());
    }
    Err(MISSING_PAYMENT_OFFER_MESSAGE.to_string())
}

/// Whether this agent has published the BOLT12 offer that paid runtime needs.
///
/// Read-only by design: pricing an agent must never provision a wallet as a
/// side effect.
async fn agent_has_payment_offer(
    state: &AppState,
    agent_pubkey: &str,
) -> Result<bool, String> {
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
    "This agent has no Bitcoin payment offer yet, so it cannot charge for runtime. \
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

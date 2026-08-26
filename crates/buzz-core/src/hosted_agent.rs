//! Payment contracts for hosted Buzz agents.
//!
//! A host advertises a fixed plan on a normal channel message. A payer buys
//! one hour by zapping that message. The buyer and host derive one agent
//! identity from the accepted zap intent.

use std::str::FromStr;

use hmac::digest::KeyInit;
use hmac::{Hmac, Mac};
use lightning_payer_proof::{verify, Offer};
use nostr::nips::nip44::v2::ConversationKey;
use nostr::{Event, EventId, JsonUtil, Keys, Kind, PublicKey, SecretKey, Tag};
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use thiserror::Error;
use uuid::Uuid;

use crate::kind::{
    KIND_BOLT12_OFFER, KIND_BOLT12_ZAP, KIND_BOLT12_ZAP_INTENT, KIND_HOSTED_AGENT_PLAN,
};

/// Stable `d` tag for the factory's plan.
pub const PLAN_IDENTIFIER: &str = "hosted-agent";
/// Tag that carries the JSON plan on a plan announcement.
pub const PLAN_TAG: &str = "agent_host_plan";
/// Tag that selects an existing lease for renewal.
pub const LEASE_TAG: &str = "lease";
/// Current plan wire version.
pub const PLAN_VERSION: u8 = 1;
/// Largest system prompt that can be advertised in one plan event.
pub const MAX_SYSTEM_PROMPT_BYTES: usize = 16 * 1024;
/// One purchased lease period.
pub const LEASE_SECONDS: u64 = 60 * 60;
/// Default time that stopped agent data remains available.
pub const DEFAULT_RETENTION_DAYS: u16 = 30;
/// Largest whole-satoshi price that stays exact after conversion to millisatoshis.
pub const MAX_HOURLY_PRICE_SATS: u64 = 9_007_199_254_740;
/// Domain separator for deterministic hosted-agent identity derivation.
pub const AGENT_KEY_DERIVATION_DOMAIN: &[u8] = b"buzz-agent-factory:agent-key:v1";
/// Stable namespace for deterministic hosted-agent lease IDs.
pub const LEASE_ID_NAMESPACE: Uuid = Uuid::from_bytes([
    0x5d, 0x63, 0xa2, 0x91, 0xe4, 0x58, 0x4d, 0xf2, 0x9b, 0xd9, 0xba, 0x47, 0xb9, 0xf0, 0x6a, 0x38,
]);

/// Public terms that a host puts in an [`PLAN_TAG`] tag.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HostedAgentPlan {
    /// Schema version. It must equal [`PLAN_VERSION`].
    pub version: u8,
    /// Stable plan name chosen by the host.
    pub name: String,
    /// Whole satoshis charged for one hour.
    pub hourly_price_sats: u64,
    /// Days that the host keeps a stopped workspace and identity.
    pub retention_days: u16,
    /// Fixed harness profile selected by the host.
    pub harness_profile: String,
    /// Fixed model selected by the host.
    pub model: String,
    /// Full host-selected system prompt.
    pub system_prompt: String,
}

impl HostedAgentPlan {
    /// Validate public plan terms.
    pub fn validate(&self) -> Result<(), HostedAgentError> {
        if self.version != PLAN_VERSION {
            return Err(HostedAgentError::UnsupportedVersion);
        }
        if self.name.trim().is_empty() || self.name.len() > 80 {
            return Err(HostedAgentError::InvalidPlan(
                "name must contain 1 to 80 bytes",
            ));
        }
        if self.hourly_price_sats == 0 || self.hourly_price_sats > MAX_HOURLY_PRICE_SATS {
            return Err(HostedAgentError::InvalidPlan(
                "hourly price is outside the supported range",
            ));
        }
        if self.retention_days == 0 || self.retention_days > 365 {
            return Err(HostedAgentError::InvalidPlan(
                "retention must contain 1 to 365 days",
            ));
        }
        if self.harness_profile.trim().is_empty() || self.harness_profile.len() > 80 {
            return Err(HostedAgentError::InvalidPlan(
                "harness profile must contain 1 to 80 bytes",
            ));
        }
        if self.model.trim().is_empty() || self.model.len() > 128 {
            return Err(HostedAgentError::InvalidPlan(
                "model must contain 1 to 128 bytes",
            ));
        }
        if self.system_prompt.trim().is_empty()
            || self.system_prompt.len() > MAX_SYSTEM_PROMPT_BYTES
        {
            return Err(HostedAgentError::InvalidPlan(
                "system prompt must contain 1 to 16384 bytes",
            ));
        }
        Ok(())
    }

    /// Encode the plan as the value of an [`PLAN_TAG`] tag.
    pub fn tag_value(&self) -> Result<String, HostedAgentError> {
        self.validate()?;
        serde_json::to_string(self).map_err(|_| HostedAgentError::InvalidPlan("plan is not JSON"))
    }
}

/// A validated request derived from a zap event.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostedAgentPurchase {
    /// Zap event ID. Hosts use it as the idempotency key.
    pub zap_event_id: String,
    /// Signed zap intent event ID. New agent identities bind to this value.
    pub intent_event_id: String,
    /// Payer profile pubkey.
    pub payer_pubkey: String,
    /// Plan message event ID.
    pub plan_event_id: String,
    /// Source channel copied from the plan message.
    pub channel_id: String,
    /// Existing lease ID for a renewal. Creation has no value.
    pub lease_id: Option<String>,
}

/// Derive the one agent identity assigned to a paid zap intent.
///
/// `local_secret` and `peer_pubkey` are the buyer/factory ECDH pair. The
/// remaining arguments have a fixed role order in the HKDF context.
pub fn derive_hosted_agent_keys(
    local_secret: &SecretKey,
    peer_pubkey: &PublicKey,
    factory_pubkey: &PublicKey,
    buyer_pubkey: &PublicKey,
    plan_event_id: &EventId,
    intent_event_id: &EventId,
) -> Result<Keys, HostedAgentError> {
    let conversation_key = ConversationKey::derive(local_secret, peer_pubkey)
        .map_err(|_| HostedAgentError::InvalidDerivation("NIP-44 ECDH failed"))?;
    let mut info = Vec::with_capacity(AGENT_KEY_DERIVATION_DOMAIN.len() + 32 * 4);
    info.extend_from_slice(AGENT_KEY_DERIVATION_DOMAIN);
    info.extend_from_slice(&factory_pubkey.to_bytes());
    info.extend_from_slice(&buyer_pubkey.to_bytes());
    info.extend_from_slice(&plan_event_id.to_bytes());
    info.extend_from_slice(&intent_event_id.to_bytes());

    // HKDF-Expand for a 32-byte output is one HMAC-SHA256 block: T(1).
    let mut expand = <Hmac<Sha256> as KeyInit>::new_from_slice(conversation_key.as_bytes())
        .map_err(|_| HostedAgentError::InvalidDerivation("HKDF key setup failed"))?;
    expand.update(&info);
    expand.update(&[1]);
    let output = expand.finalize().into_bytes();
    let secret = SecretKey::from_slice(&output)
        .map_err(|_| HostedAgentError::InvalidDerivation("derived invalid secp256k1 key"))?;
    Ok(Keys::new(secret))
}

/// Derive the stable lease ID for a hosted-agent identity.
pub fn derive_hosted_agent_lease_id(agent_pubkey: &PublicKey) -> Uuid {
    Uuid::new_v5(&LEASE_ID_NAMESPACE, &agent_pubkey.to_bytes())
}

/// Parse and validate a parameterized-replaceable host plan.
pub fn plan_from_event(
    event: &Event,
    host_pubkey: &str,
) -> Result<HostedAgentPlan, HostedAgentError> {
    event
        .verify()
        .map_err(|_| HostedAgentError::InvalidPlanEvent)?;
    if event.kind != Kind::Custom(KIND_HOSTED_AGENT_PLAN as u16)
        || event.pubkey.to_hex() != host_pubkey
    {
        return Err(HostedAgentError::InvalidPlanEvent);
    }
    let value = exact_tag(event, PLAN_TAG)?;
    exact_tag(event, "h")?;
    if exact_tag(event, "d")? != PLAN_IDENTIFIER {
        return Err(HostedAgentError::InvalidPlanEvent);
    }
    let plan: HostedAgentPlan = serde_json::from_str(value)
        .map_err(|_| HostedAgentError::InvalidPlan("plan tag is not valid JSON"))?;
    plan.validate()?;
    Ok(plan)
}

/// Validate one creation or renewal zap against its active plan message.
///
pub fn validate_purchase_zap(
    zap: &Event,
    plan_event: &Event,
    host_pubkey: &str,
) -> Result<HostedAgentPurchase, HostedAgentError> {
    let plan = plan_from_event(plan_event, host_pubkey)?;
    zap.verify()
        .map_err(|_| HostedAgentError::InvalidZap("bad signature"))?;
    if zap.kind != Kind::Custom(KIND_BOLT12_ZAP as u16) {
        return Err(HostedAgentError::InvalidZap("wrong event kind"));
    }
    if exact_tag(zap, "p")? != host_pubkey {
        return Err(HostedAgentError::InvalidZap("wrong host"));
    }
    if exact_tag(zap, "e")? != plan_event.id.to_hex() {
        return Err(HostedAgentError::InvalidZap("wrong plan"));
    }
    let channel_id = exact_tag(plan_event, "h")?;
    if exact_tag(zap, "h")? != channel_id {
        return Err(HostedAgentError::InvalidZap("wrong channel"));
    }
    let expected_msats = plan
        .hourly_price_sats
        .checked_mul(1_000)
        .ok_or(HostedAgentError::InvalidZap("price overflow"))?;
    if exact_tag(zap, "amount")?.parse::<u64>().ok() != Some(expected_msats) {
        return Err(HostedAgentError::InvalidZap("wrong hourly amount"));
    }

    let intent = Event::from_json(exact_tag(zap, "description")?)
        .map_err(|_| HostedAgentError::InvalidZap("bad embedded intent"))?;
    intent
        .verify()
        .map_err(|_| HostedAgentError::InvalidZap("bad intent signature"))?;
    if intent.kind != Kind::Custom(KIND_BOLT12_ZAP_INTENT as u16) || intent.pubkey != zap.pubkey {
        return Err(HostedAgentError::InvalidZap("intent does not match payer"));
    }
    if exact_tag(zap, "P")? != zap.pubkey.to_hex() || zap.content != intent.content {
        return Err(HostedAgentError::InvalidZap("proof does not match payer"));
    }
    for name in ["p", "e", "h", "amount", "offer_event"] {
        if exact_tag(&intent, name)? != exact_tag(zap, name)? {
            return Err(HostedAgentError::InvalidZap(
                "intent tags do not match proof",
            ));
        }
    }
    let offer_event = Event::from_json(exact_tag(zap, "offer_event")?)
        .map_err(|_| HostedAgentError::InvalidZap("bad offer announcement"))?;
    offer_event
        .verify()
        .map_err(|_| HostedAgentError::InvalidZap("bad offer signature"))?;
    if offer_event.kind != Kind::Custom(KIND_BOLT12_OFFER as u16)
        || offer_event.pubkey.to_hex() != host_pubkey
    {
        return Err(HostedAgentError::InvalidZap(
            "offer does not belong to host",
        ));
    }
    let proof_text = exact_tag(zap, "proof")?;
    if !proof_text.starts_with("lnp1")
        || proof_text != proof_text.to_ascii_lowercase()
        || proof_text.contains(['+', '\n', '\r', ' ', '\t'])
    {
        return Err(HostedAgentError::InvalidZap(
            "payer proof is not canonically encoded",
        ));
    }
    let proof =
        verify(proof_text).map_err(|_| HostedAgentError::InvalidZap("invalid payer proof"))?;
    let expected_proof_note = format!("nostr:nipB1:{}", intent.id.to_hex());
    if proof.proof_note().map(|note| note.0).as_deref() != Some(expected_proof_note.as_str()) {
        return Err(HostedAgentError::InvalidZap(
            "payer proof does not name the signed intent",
        ));
    }
    if proof.invoice_amount_msats() != Some(expected_msats) {
        return Err(HostedAgentError::InvalidZap(
            "payer proof does not match the hourly amount",
        ));
    }
    let mut expected_tags = intent
        .tags
        .iter()
        .filter(|tag| tag.as_slice().first().map(String::as_str) != Some("zap_id"))
        .cloned()
        .collect::<Vec<_>>();
    expected_tags.extend([
        Tag::parse(["description", intent.as_json().as_str()])
            .map_err(|_| HostedAgentError::InvalidZap("invalid canonical description tag"))?,
        Tag::parse(["P", zap.pubkey.to_hex().as_str()])
            .map_err(|_| HostedAgentError::InvalidZap("invalid canonical payer tag"))?,
        Tag::parse(["proof", proof_text])
            .map_err(|_| HostedAgentError::InvalidZap("invalid canonical proof tag"))?,
    ]);
    if !zap.tags.iter().eq(expected_tags.iter()) {
        return Err(HostedAgentError::InvalidZap(
            "zap is not the canonical envelope for its intent and proof",
        ));
    }
    let offers = offer_event
        .tags
        .iter()
        .filter_map(|tag| {
            let parts = tag.as_slice();
            (parts.first().map(String::as_str) == Some("offer"))
                .then(|| parts.get(1).map(String::as_str))
                .flatten()
        })
        .filter_map(|value| {
            let offer = Offer::from_str(value).ok()?;
            (offer.to_string() == value).then_some(offer)
        })
        .collect::<Vec<_>>();
    if offers.is_empty()
        || !offers
            .iter()
            .any(|offer| proof.pays_offers_recipient(offer))
    {
        return Err(HostedAgentError::InvalidZap(
            "payer proof does not pay the host offer recipient",
        ));
    }
    let lease_id = matching_optional_tag(zap, &intent, LEASE_TAG)?
        .map(str::to_string)
        .filter(|value| !value.trim().is_empty());

    Ok(HostedAgentPurchase {
        zap_event_id: zap.id.to_hex(),
        intent_event_id: intent.id.to_hex(),
        payer_pubkey: zap.pubkey.to_hex(),
        plan_event_id: plan_event.id.to_hex(),
        channel_id: channel_id.to_string(),
        lease_id,
    })
}

fn exact_tag<'a>(event: &'a Event, name: &str) -> Result<&'a str, HostedAgentError> {
    let values = event
        .tags
        .iter()
        .filter(|tag| tag.as_slice().first().map(String::as_str) == Some(name))
        .collect::<Vec<_>>();
    if values.len() != 1 {
        return Err(HostedAgentError::InvalidZap("missing or duplicate tag"));
    }
    values[0]
        .as_slice()
        .get(1)
        .map(String::as_str)
        .ok_or(HostedAgentError::InvalidZap("tag has no value"))
}

fn optional_tag<'a>(event: &'a Event, name: &str) -> Result<Option<&'a str>, HostedAgentError> {
    let values = event
        .tags
        .iter()
        .filter(|tag| tag.as_slice().first().map(String::as_str) == Some(name))
        .collect::<Vec<_>>();
    if values.len() > 1 {
        return Err(HostedAgentError::InvalidZap("duplicate optional tag"));
    }
    values
        .first()
        .map(|tag| {
            tag.as_slice()
                .get(1)
                .map(String::as_str)
                .ok_or(HostedAgentError::InvalidZap("tag has no value"))
        })
        .transpose()
}

fn matching_optional_tag<'a>(
    outer: &'a Event,
    intent: &'a Event,
    name: &str,
) -> Result<Option<&'a str>, HostedAgentError> {
    let outer_value = optional_tag(outer, name)?;
    if outer_value != optional_tag(intent, name)? {
        return Err(HostedAgentError::InvalidZap(
            "optional intent tag does not match proof",
        ));
    }
    Ok(outer_value)
}

/// Hosted-agent contract error.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum HostedAgentError {
    /// Unknown plan schema version.
    #[error("unsupported hosted-agent plan version")]
    UnsupportedVersion,
    /// Invalid public plan terms.
    #[error("invalid hosted-agent plan: {0}")]
    InvalidPlan(&'static str),
    /// The event is not a signed host plan message.
    #[error("invalid hosted-agent plan event")]
    InvalidPlanEvent,
    /// The zap does not purchase the selected plan.
    #[error("invalid hosted-agent zap: {0}")]
    InvalidZap(&'static str),
    /// The shared buyer/factory inputs could not produce an agent identity.
    #[error("invalid hosted-agent identity derivation: {0}")]
    InvalidDerivation(&'static str),
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::payer_proof_test_utils::payer_proof_for_note;
    use nostr::{EventBuilder, Keys, SecretKey, Tag, Timestamp};

    #[derive(Deserialize)]
    struct DerivationVector {
        buyer_secret: String,
        factory_secret: String,
        plan_event_id: String,
        intent_event_id: String,
        conversation_key: String,
        agent_secret: String,
        agent_pubkey: String,
        lease_id: String,
    }

    #[test]
    fn hosted_agent_derivation_matches_shared_vectors() {
        let vectors: Vec<DerivationVector> =
            serde_json::from_str(include_str!("../test-vectors/hosted-agent-derivation.json"))
                .unwrap();
        for vector in vectors {
            let buyer = Keys::new(SecretKey::from_hex(&vector.buyer_secret).unwrap());
            let factory = Keys::new(SecretKey::from_hex(&vector.factory_secret).unwrap());
            let plan = EventId::from_hex(&vector.plan_event_id).unwrap();
            let intent = EventId::from_hex(&vector.intent_event_id).unwrap();
            let conversation_key =
                ConversationKey::derive(buyer.secret_key(), &factory.public_key()).unwrap();
            let buyer_result = derive_hosted_agent_keys(
                buyer.secret_key(),
                &factory.public_key(),
                &factory.public_key(),
                &buyer.public_key(),
                &plan,
                &intent,
            )
            .unwrap();
            let factory_result = derive_hosted_agent_keys(
                factory.secret_key(),
                &buyer.public_key(),
                &factory.public_key(),
                &buyer.public_key(),
                &plan,
                &intent,
            )
            .unwrap();

            assert_eq!(buyer_result.secret_key(), factory_result.secret_key());
            assert_eq!(
                hex::encode(conversation_key.as_bytes()),
                vector.conversation_key
            );
            assert_eq!(
                buyer_result.secret_key().to_secret_hex(),
                vector.agent_secret
            );
            assert_eq!(buyer_result.public_key().to_hex(), vector.agent_pubkey);
            assert_eq!(
                derive_hosted_agent_lease_id(&buyer_result.public_key()).to_string(),
                vector.lease_id
            );
        }
    }

    #[test]
    fn each_derivation_context_field_changes_the_agent() {
        let buyer = Keys::new(SecretKey::from_slice(&[3; 32]).unwrap());
        let other_buyer = Keys::new(SecretKey::from_slice(&[5; 32]).unwrap());
        let factory = Keys::new(SecretKey::from_slice(&[4; 32]).unwrap());
        let other_factory = Keys::new(SecretKey::from_slice(&[6; 32]).unwrap());
        let plan = EventId::from_slice(&[7; 32]).unwrap();
        let other_plan = EventId::from_slice(&[8; 32]).unwrap();
        let intent = EventId::from_slice(&[9; 32]).unwrap();
        let other_intent = EventId::from_slice(&[10; 32]).unwrap();
        let derive = |buyer: &Keys, factory: &Keys, plan: &EventId, intent: &EventId| {
            derive_hosted_agent_keys(
                buyer.secret_key(),
                &factory.public_key(),
                &factory.public_key(),
                &buyer.public_key(),
                plan,
                intent,
            )
            .unwrap()
            .public_key()
        };
        let baseline = derive(&buyer, &factory, &plan, &intent);

        assert_ne!(baseline, derive(&other_buyer, &factory, &plan, &intent));
        assert_ne!(baseline, derive(&buyer, &other_factory, &plan, &intent));
        assert_ne!(baseline, derive(&buyer, &factory, &other_plan, &intent));
        assert_ne!(baseline, derive(&buyer, &factory, &plan, &other_intent));
    }

    fn valid_offer() -> String {
        payer_proof_for_note("").0
    }

    fn tag(values: &[&str]) -> Tag {
        Tag::parse(values.iter().copied()).unwrap()
    }

    fn plan_event(host: &Keys) -> Event {
        let plan = HostedAgentPlan {
            version: PLAN_VERSION,
            name: "Standard".into(),
            hourly_price_sats: 42,
            retention_days: DEFAULT_RETENTION_DAYS,
            harness_profile: "buzz-default".into(),
            model: "test-model".into(),
            system_prompt: "You are a test agent.".into(),
        };
        EventBuilder::new(
            Kind::Custom(KIND_HOSTED_AGENT_PLAN as u16),
            "Hosted agent: 42 sats/hour",
        )
        .tags([
            tag(&["h", "7d30ff53-e846-4f3f-8cb4-1bb8784bf399"]),
            tag(&["d", PLAN_IDENTIFIER]),
            tag(&[PLAN_TAG, &plan.tag_value().unwrap()]),
        ])
        .sign_with_keys(host)
        .unwrap()
    }

    fn zap(host: &Keys, payer: &Keys, plan: &Event, lease: Option<&str>) -> Event {
        zap_with(host, payer, plan, lease, "42000", None)
    }

    fn zap_with(
        host: &Keys,
        payer: &Keys,
        plan: &Event,
        lease: Option<&str>,
        amount: &str,
        proof: Option<&str>,
    ) -> Event {
        let host_hex = host.public_key().to_hex();
        let plan_hex = plan.id.to_hex();
        let channel = exact_tag(plan, "h").unwrap();
        let mut tags = vec![
            tag(&["p", &host_hex]),
            tag(&["e", &plan_hex]),
            tag(&["h", channel]),
            tag(&["amount", amount]),
            tag(&[
                "offer_event",
                &EventBuilder::new(Kind::Custom(KIND_BOLT12_OFFER as u16), "")
                    .tags([
                        tag(&["offer", "invalid-offer"]),
                        tag(&["offer", &valid_offer()]),
                    ])
                    .sign_with_keys(host)
                    .unwrap()
                    .as_json(),
            ]),
        ];
        if let Some(lease) = lease {
            tags.push(tag(&[LEASE_TAG, lease]));
        }
        let intent = EventBuilder::new(Kind::Custom(KIND_BOLT12_ZAP_INTENT as u16), "")
            .tags(tags.clone())
            .sign_with_keys(payer)
            .unwrap();
        let generated_proof;
        let proof = match proof {
            Some(proof) => proof,
            None => {
                generated_proof =
                    payer_proof_for_note(&format!("nostr:nipB1:{}", intent.id.to_hex())).1;
                &generated_proof
            }
        };
        tags.extend([
            tag(&["description", &intent.as_json()]),
            tag(&["P", &payer.public_key().to_hex()]),
            tag(&["proof", proof]),
        ]);
        EventBuilder::new(Kind::Custom(KIND_BOLT12_ZAP as u16), "")
            .tags(tags)
            .sign_with_keys(payer)
            .unwrap()
    }

    #[test]
    fn accepts_creation_and_renewal_from_the_normal_profile() {
        let host = Keys::generate();
        let payer = Keys::generate();
        let plan = plan_event(&host);
        let creation = validate_purchase_zap(
            &zap(&host, &payer, &plan, None),
            &plan,
            &host.public_key().to_hex(),
        )
        .unwrap();
        assert_eq!(creation.payer_pubkey, payer.public_key().to_hex());
        assert_eq!(creation.lease_id, None);
        let renewal = validate_purchase_zap(
            &zap(&host, &payer, &plan, Some("lease-1")),
            &plan,
            &host.public_key().to_hex(),
        )
        .unwrap();
        assert_eq!(renewal.lease_id.as_deref(), Some("lease-1"));
    }

    #[test]
    fn plan_requires_the_replaceable_kind_and_address() {
        let host = Keys::generate();
        let valid = plan_event(&host);
        assert!(plan_from_event(&valid, &host.public_key().to_hex()).is_ok());

        let wrong_address = EventBuilder::new(
            Kind::Custom(KIND_HOSTED_AGENT_PLAN as u16),
            valid.content.clone(),
        )
        .tags(
            valid
                .tags
                .iter()
                .filter(|tag| tag.as_slice().first().map(String::as_str) != Some("d"))
                .cloned()
                .chain(std::iter::once(tag(&["d", "wrong-plan"]))),
        )
        .sign_with_keys(&host)
        .unwrap();
        assert!(plan_from_event(&wrong_address, &host.public_key().to_hex()).is_err());
    }

    #[test]
    fn rejects_invalid_proofs_and_wrong_amounts() {
        let host = Keys::generate();
        let payer = Keys::generate();
        let plan = plan_event(&host);
        let host_hex = host.public_key().to_hex();
        let wrong_proof = zap_with(&host, &payer, &plan, None, "42000", Some("wallet-proof"));
        assert!(validate_purchase_zap(&wrong_proof, &plan, &host_hex).is_err());
        let wrong_amount = zap_with(&host, &payer, &plan, None, "41000", None);
        assert!(validate_purchase_zap(&wrong_amount, &plan, &host_hex).is_err());
    }

    #[test]
    fn rejects_proof_bound_to_another_intent() {
        let host = Keys::generate();
        let payer = Keys::generate();
        let plan = plan_event(&host);
        let (_, wrong_proof, _) = payer_proof_for_note(&format!("nostr:nipB1:{}", "00".repeat(32)));
        let zap = zap_with(&host, &payer, &plan, None, "42000", Some(&wrong_proof));
        assert!(validate_purchase_zap(&zap, &plan, &host.public_key().to_hex()).is_err());
    }

    #[test]
    fn accepts_an_outer_timestamp_independent_of_the_invoice() {
        let host = Keys::generate();
        let payer = Keys::generate();
        let plan = plan_event(&host);
        let canonical = zap(&host, &payer, &plan, None);
        let altered = EventBuilder::new(canonical.kind, canonical.content.clone())
            .tags(canonical.tags.iter().cloned())
            .custom_created_at(Timestamp::from(canonical.created_at.as_secs() + 1))
            .sign_with_keys(&payer)
            .unwrap();

        assert!(validate_purchase_zap(&altered, &plan, &host.public_key().to_hex()).is_ok());
    }

    #[test]
    fn plan_defaults_to_thirty_day_retention() {
        assert_eq!(DEFAULT_RETENTION_DAYS, 30);
        assert_eq!(LEASE_SECONDS, 3600);
    }

    #[test]
    fn version_one_requires_full_agent_configuration() {
        let mut current = HostedAgentPlan {
            version: PLAN_VERSION,
            name: "Standard".into(),
            hourly_price_sats: 500,
            retention_days: DEFAULT_RETENTION_DAYS,
            harness_profile: "codex".into(),
            model: String::new(),
            system_prompt: "Be helpful.".into(),
        };
        assert!(current.validate().is_err());
        current.model = "test-model".into();
        current.system_prompt.clear();
        assert!(current.validate().is_err());
        current.system_prompt = "Be helpful.".into();
        assert!(current.validate().is_ok());
    }
}

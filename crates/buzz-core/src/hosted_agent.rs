//! Payment contracts for hosted Buzz agents.
//!
//! A host advertises a fixed plan on a normal channel message. A payer buys
//! one hour by zapping that message. The host creates the agent identity only
//! after it accepts the zap.

use std::str::FromStr;

use nostr::secp256k1::{schnorr::Signature, Message, SECP256K1};
use nostr::{Event, JsonUtil, Keys, Kind, PublicKey, Tag};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;
use uuid::Uuid;

use crate::kind::{
    KIND_BOLT12_OFFER, KIND_BOLT12_ZAP, KIND_BOLT12_ZAP_INTENT, KIND_STREAM_MESSAGE_V2,
};

/// Tag that carries the JSON plan on a channel message.
pub const PLAN_TAG: &str = "agent_host_plan";
/// Tag that selects an existing lease for renewal.
pub const LEASE_TAG: &str = "lease";
/// Host-signed profile tag that identifies the buyer who manages an agent.
pub const CONTROLLER_TAG: &str = "hosted_agent_controller";
/// The temporary proof accepted by the host until wallet settlement checks exist.
pub const PLACEHOLDER_PROOF: &str = "placeholder";
/// Current plan wire version.
pub const PLAN_VERSION: u8 = 1;
/// One purchased lease period.
pub const LEASE_SECONDS: u64 = 60 * 60;
/// Default time that stopped agent data remains available.
pub const DEFAULT_RETENTION_DAYS: u16 = 30;
/// Largest whole-satoshi price that stays exact after conversion to millisatoshis.
pub const MAX_HOURLY_PRICE_SATS: u64 = 9_007_199_254_740;

fn controller_preimage(
    agent_pubkey: &PublicKey,
    controller_pubkey: &PublicKey,
    host_pubkey: &PublicKey,
    plan_event_id: &str,
    lease_id: &str,
) -> String {
    format!(
        "buzz:hosted-agent-controller:{}:{}:{}:{plan_event_id}:{lease_id}",
        agent_pubkey.to_hex(),
        controller_pubkey.to_hex(),
        host_pubkey.to_hex(),
    )
}

/// Build a host-signed profile tag for the buyer who manages an agent.
///
/// This tag is for display and lease control. It does not replace the NIP-OA
/// owner proof and must not grant owner-only permissions.
pub fn build_controller_tag(
    host_keys: &Keys,
    agent_pubkey: &PublicKey,
    controller_pubkey: &str,
    plan_event_id: &str,
    lease_id: &str,
) -> Result<Tag, HostedAgentError> {
    let controller = PublicKey::from_hex(controller_pubkey)
        .map_err(|_| HostedAgentError::InvalidControllerTag)?;
    if plan_event_id.len() != 64
        || !plan_event_id.bytes().all(|byte| byte.is_ascii_hexdigit())
        || Uuid::parse_str(lease_id).is_err()
    {
        return Err(HostedAgentError::InvalidControllerTag);
    }
    let host = host_keys.public_key();
    let digest = Sha256::digest(
        controller_preimage(agent_pubkey, &controller, &host, plan_event_id, lease_id).as_bytes(),
    );
    let signature = host_keys.sign_schnorr(&Message::from_digest(digest.into()));
    Tag::parse([
        CONTROLLER_TAG,
        controller_pubkey,
        &host.to_hex(),
        plan_event_id,
        lease_id,
        &signature.to_string(),
    ])
    .map_err(|_| HostedAgentError::InvalidControllerTag)
}

/// Verify a hosted-agent controller tag against its NIP-OA host owner.
///
/// The returned key is a display-only lease controller. It is not the
/// cryptographic owner of the agent.
pub fn verify_controller_tag(
    values: &[String],
    agent_pubkey: &PublicKey,
    expected_host: &PublicKey,
) -> Result<PublicKey, HostedAgentError> {
    if values.len() != 6
        || values.first().map(String::as_str) != Some(CONTROLLER_TAG)
        || values.get(2).map(String::as_str) != Some(expected_host.to_hex().as_str())
    {
        return Err(HostedAgentError::InvalidControllerTag);
    }
    let controller =
        PublicKey::from_hex(&values[1]).map_err(|_| HostedAgentError::InvalidControllerTag)?;
    if values[3].len() != 64
        || !values[3].bytes().all(|byte| byte.is_ascii_hexdigit())
        || Uuid::parse_str(&values[4]).is_err()
    {
        return Err(HostedAgentError::InvalidControllerTag);
    }
    let signature =
        Signature::from_str(&values[5]).map_err(|_| HostedAgentError::InvalidControllerTag)?;
    let digest = Sha256::digest(
        controller_preimage(
            agent_pubkey,
            &controller,
            expected_host,
            &values[3],
            &values[4],
        )
        .as_bytes(),
    );
    let host = expected_host
        .xonly()
        .map_err(|_| HostedAgentError::InvalidControllerTag)?;
    SECP256K1
        .verify_schnorr(&signature, &Message::from_digest(digest.into()), &host)
        .map_err(|_| HostedAgentError::InvalidControllerTag)?;
    Ok(controller)
}

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
    /// Payer profile pubkey.
    pub payer_pubkey: String,
    /// Plan message event ID.
    pub plan_event_id: String,
    /// Source channel copied from the plan message.
    pub channel_id: String,
    /// Existing lease ID for a renewal. Creation has no value.
    pub lease_id: Option<String>,
}

/// Parse and validate a host plan from a normal channel message.
pub fn plan_from_event(
    event: &Event,
    host_pubkey: &str,
) -> Result<HostedAgentPlan, HostedAgentError> {
    event
        .verify()
        .map_err(|_| HostedAgentError::InvalidPlanEvent)?;
    if event.kind != Kind::Custom(KIND_STREAM_MESSAGE_V2 as u16)
        || event.pubkey.to_hex() != host_pubkey
    {
        return Err(HostedAgentError::InvalidPlanEvent);
    }
    let value = exact_tag(event, PLAN_TAG)?;
    exact_tag(event, "h")?;
    let plan: HostedAgentPlan = serde_json::from_str(value)
        .map_err(|_| HostedAgentError::InvalidPlan("plan tag is not valid JSON"))?;
    plan.validate()?;
    Ok(plan)
}

/// Validate one creation or renewal zap against its active plan message.
///
/// This temporary validator deliberately accepts only the literal
/// [`PLACEHOLDER_PROOF`]. It does not query the receiving wallet. A later
/// protocol revision must replace this rule with settlement verification.
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
    if exact_tag(zap, "proof")? != PLACEHOLDER_PROOF {
        return Err(HostedAgentError::InvalidZap(
            "only the temporary placeholder proof is accepted",
        ));
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
    let lease_id = matching_optional_tag(zap, &intent, LEASE_TAG)?
        .map(str::to_string)
        .filter(|value| !value.trim().is_empty());

    Ok(HostedAgentPurchase {
        zap_event_id: zap.id.to_hex(),
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
    /// The hosted-agent controller tag is malformed or has a bad signature.
    #[error("invalid hosted-agent controller tag")]
    InvalidControllerTag,
}

#[cfg(test)]
mod tests {
    use super::*;
    use nostr::{EventBuilder, Keys, Tag};

    fn tag(values: &[&str]) -> Tag {
        Tag::parse(values.iter().copied()).unwrap()
    }

    fn plan_event(host: &Keys) -> Event {
        let plan = HostedAgentPlan {
            version: PLAN_VERSION,
            name: "Standard".into(),
            hourly_price_sats: 500,
            retention_days: DEFAULT_RETENTION_DAYS,
            harness_profile: "buzz-default".into(),
        };
        EventBuilder::new(
            Kind::Custom(KIND_STREAM_MESSAGE_V2 as u16),
            "Hosted agent: 500 sats/hour",
        )
        .tags([
            tag(&["h", "7d30ff53-e846-4f3f-8cb4-1bb8784bf399"]),
            tag(&[PLAN_TAG, &plan.tag_value().unwrap()]),
        ])
        .sign_with_keys(host)
        .unwrap()
    }

    fn zap(host: &Keys, payer: &Keys, plan: &Event, lease: Option<&str>) -> Event {
        zap_with(host, payer, plan, lease, "500000", PLACEHOLDER_PROOF)
    }

    fn zap_with(
        host: &Keys,
        payer: &Keys,
        plan: &Event,
        lease: Option<&str>,
        amount: &str,
        proof: &str,
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
                    .tag(tag(&["offer", "lno1test"]))
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
    fn rejects_non_placeholder_proofs_and_wrong_amounts() {
        let host = Keys::generate();
        let payer = Keys::generate();
        let plan = plan_event(&host);
        let host_hex = host.public_key().to_hex();
        let wrong_proof = zap_with(&host, &payer, &plan, None, "500000", "wallet-proof");
        assert!(validate_purchase_zap(&wrong_proof, &plan, &host_hex).is_err());
        let wrong_amount = zap_with(&host, &payer, &plan, None, "499000", PLACEHOLDER_PROOF);
        assert!(validate_purchase_zap(&wrong_amount, &plan, &host_hex).is_err());
    }

    #[test]
    fn plan_defaults_to_thirty_day_retention() {
        assert_eq!(DEFAULT_RETENTION_DAYS, 30);
        assert_eq!(LEASE_SECONDS, 3600);
    }

    #[test]
    fn controller_tag_binds_buyer_agent_host_and_lease() {
        let host = Keys::generate();
        let agent = Keys::generate();
        let buyer = Keys::generate();
        let plan = "a".repeat(64);
        let lease = Uuid::new_v4().to_string();
        let tag = build_controller_tag(
            &host,
            &agent.public_key(),
            &buyer.public_key().to_hex(),
            &plan,
            &lease,
        )
        .unwrap();

        let controller =
            verify_controller_tag(tag.as_slice(), &agent.public_key(), &host.public_key()).unwrap();
        assert_eq!(controller, buyer.public_key());

        let wrong_agent = Keys::generate();
        assert!(verify_controller_tag(
            tag.as_slice(),
            &wrong_agent.public_key(),
            &host.public_key(),
        )
        .is_err());
    }
}

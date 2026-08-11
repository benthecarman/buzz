//! Agent-side validation for paid invocation zaps.

use std::{collections::HashSet, str::FromStr};

use buzz_core::{
    agent_runtime_payment::RuntimePricing,
    kind::{
        KIND_AGENT_RUNTIME_PRICING, KIND_BOLT12_OFFER, KIND_BOLT12_ZAP, KIND_BOLT12_ZAP_INTENT,
        KIND_STREAM_MESSAGE,
    },
};
use lightning::offers::{offer::Offer, payer_proof::PayerProof};
use nostr::{Event, EventId, JsonUtil, Kind, PublicKey};

use crate::{
    config::{Config, RespondTo},
    relay::RestClient,
};

const PLACEHOLDER_PAYER_PROOF: &str = "placeholder";

fn protocol_error(message: impl Into<String>) -> anyhow::Error {
    anyhow::anyhow!(message.into())
}

pub(crate) fn payer_has_paid_access(
    respond_to: &RespondTo,
    allowlist: &HashSet<String>,
    payer_hex: &str,
) -> bool {
    match respond_to {
        RespondTo::Anyone => true,
        RespondTo::Allowlist => allowlist.contains(payer_hex),
        RespondTo::OwnerOnly | RespondTo::Nobody => false,
    }
}

fn exactly_one_tag<'a>(event: &'a Event, name: &str) -> anyhow::Result<&'a str> {
    let values = event
        .tags
        .iter()
        .filter_map(|tag| {
            let parts = tag.as_slice();
            parts
                .first()
                .is_some_and(|value| value.as_str() == name)
                .then_some(parts)
        })
        .collect::<Vec<_>>();
    match values.as_slice() {
        [parts] if parts.len() == 2 => Ok(parts[1].as_str()),
        _ => Err(protocol_error(format!("expected exactly one {name} tag"))),
    }
}

fn optional_tag<'a>(event: &'a Event, name: &str) -> anyhow::Result<Option<&'a str>> {
    let values = event
        .tags
        .iter()
        .filter_map(|tag| {
            let parts = tag.as_slice();
            parts
                .first()
                .is_some_and(|value| value.as_str() == name)
                .then_some(parts)
        })
        .collect::<Vec<_>>();
    match values.as_slice() {
        [] => Ok(None),
        [parts] if parts.len() == 2 => Ok(Some(parts[1].as_str())),
        _ => Err(protocol_error(format!("multiple {name} tags"))),
    }
}

fn runtime_zap_id(event: &Event, agent_hex: &str) -> anyhow::Result<String> {
    let values = event
        .tags
        .iter()
        .filter_map(|tag| {
            let parts = tag.as_slice();
            (parts
                .first()
                .is_some_and(|value| value.as_str() == "agent_runtime"))
            .then_some(parts)
        })
        .collect::<Vec<_>>();
    match values.as_slice() {
        [parts]
            if parts.len() == 3
                && parts[1].as_str() == agent_hex
                && EventId::from_hex(parts[2].as_str()).is_ok() =>
        {
            Ok(parts[2].to_string())
        }
        [_] => Err(protocol_error("malformed agent_runtime tag")),
        [] => Err(protocol_error("paid instruction has no agent_runtime tag")),
        _ => Err(protocol_error(
            "paid instruction has multiple agent_runtime tags",
        )),
    }
}

async fn query_events(rest: &RestClient, filter: nostr::Filter) -> anyhow::Result<Vec<Event>> {
    let value = rest.query(&[filter]).await?;
    value
        .as_array()
        .ok_or_else(|| protocol_error("relay query did not return an event array"))?
        .iter()
        .map(|row| Event::from_json(row.to_string()).map_err(Into::into))
        .collect()
}

fn validate_offer_event(event: &Event, agent: PublicKey) -> anyhow::Result<()> {
    event.verify()?;
    if event.kind != Kind::Custom(KIND_BOLT12_OFFER as u16) || event.pubkey != agent {
        return Err(protocol_error("zap offer does not belong to the Agent"));
    }
    let offer = exactly_one_tag(event, "offer")?;
    let parsed = Offer::from_str(offer).map_err(|_| protocol_error("zap offer is malformed"))?;
    if parsed.to_string() != offer {
        return Err(protocol_error("zap offer is not canonical"));
    }
    Ok(())
}

fn validate_zap_chain(
    zap: &Event,
    instruction: &Event,
    pricing_event: &Event,
    channel_id: &str,
    agent: PublicKey,
) -> anyhow::Result<()> {
    instruction.verify()?;
    if instruction.kind != Kind::Custom(KIND_STREAM_MESSAGE as u16) {
        return Err(protocol_error("paid instruction must be kind 9"));
    }
    if exactly_one_tag(instruction, "h")? != channel_id {
        return Err(protocol_error(
            "instruction channel does not match the active channel",
        ));
    }
    zap.verify()?;
    pricing_event.verify()?;
    if zap.kind != Kind::Custom(KIND_BOLT12_ZAP as u16)
        || zap.pubkey != instruction.pubkey
        || exactly_one_tag(zap, "P")? != zap.pubkey.to_hex()
        || exactly_one_tag(zap, "p")? != agent.to_hex()
        || exactly_one_tag(zap, "h")? != channel_id
        || exactly_one_tag(zap, "e")? != pricing_event.id.to_hex()
    {
        return Err(protocol_error("zap routing does not match the instruction"));
    }
    if pricing_event.kind != Kind::Custom(KIND_AGENT_RUNTIME_PRICING as u16)
        || pricing_event.pubkey != agent
    {
        return Err(protocol_error("zap target is not Agent pricing"));
    }
    let pricing: RuntimePricing = serde_json::from_str(&pricing_event.content)?;
    pricing.validate()?;
    if !pricing.enabled {
        return Err(protocol_error("Agent pricing is disabled"));
    }
    let price_sats = pricing
        .price_sats
        .ok_or_else(|| protocol_error("Agent pricing has no price"))?;
    let window = pricing
        .invocation_window_seconds
        .ok_or_else(|| protocol_error("Agent pricing has no invocation window"))?;
    let amount_msats = exactly_one_tag(zap, "amount")?
        .parse::<u64>()
        .map_err(|_| protocol_error("zap amount is not an integer"))?;
    if amount_msats != price_sats.saturating_mul(1_000) {
        return Err(protocol_error("zap amount does not match Agent pricing"));
    }
    let proof = exactly_one_tag(zap, "proof")?;
    if proof != PLACEHOLDER_PAYER_PROOF {
        let proof = PayerProof::from_str(proof)
            .map_err(|_| protocol_error("BOLT12 payer proof is malformed"))?;
        if proof
            .invoice_amount_msats()
            .is_some_and(|proof_amount| proof_amount != amount_msats)
        {
            return Err(protocol_error("payer proof amount does not match the zap"));
        }
    }
    let intent = Event::from_json(exactly_one_tag(zap, "description")?)?;
    intent.verify()?;
    if intent.kind != Kind::Custom(KIND_BOLT12_ZAP_INTENT as u16)
        || intent.pubkey != zap.pubkey
        || intent.content != zap.content
    {
        return Err(protocol_error("zap does not match its signed intent"));
    }
    for name in ["p", "e", "h", "amount", "offer_event"] {
        if exactly_one_tag(&intent, name)? != exactly_one_tag(zap, name)? {
            return Err(protocol_error(format!(
                "zap {name} tag does not match its intent"
            )));
        }
    }
    if optional_tag(&intent, "k")?.is_some() || optional_tag(zap, "k")?.is_some() {
        return Err(protocol_error("runtime zap must not contain a k tag"));
    }
    let offer_event = Event::from_json(exactly_one_tag(&intent, "offer_event")?)?;
    validate_offer_event(&offer_event, agent)?;
    let starts_at = instruction.created_at.as_secs();
    let zap_time = zap.created_at.as_secs();
    if starts_at < zap_time || starts_at > zap_time.saturating_add(window) {
        return Err(protocol_error("zap invocation window expired"));
    }
    Ok(())
}

pub struct PaidRuntimeTerms {
    pub keys: nostr::Keys,
    pub respond_to: RespondTo,
    pub respond_to_allowlist: HashSet<String>,
    pub priced: bool,
}

impl PaidRuntimeTerms {
    pub fn from_config(config: &Config) -> Self {
        Self {
            keys: config.keys.clone(),
            respond_to: config.respond_to.clone(),
            respond_to_allowlist: config.respond_to_allowlist.clone(),
            priced: config.price_per_minute_sats.is_some(),
        }
    }
}

pub fn kill_switch_active() -> bool {
    std::env::var("BUZZ_ACP_DISABLE_PAID_RUNTIME")
        .ok()
        .is_some_and(|value| {
            matches!(
                value.trim().to_ascii_lowercase().as_str(),
                "1" | "true" | "yes" | "on"
            )
        })
}

pub async fn validate_pricing_readiness(rest: &RestClient, agent: PublicKey) -> anyhow::Result<()> {
    let events = query_events(
        rest,
        nostr::Filter::new()
            .author(agent)
            .kind(Kind::Custom(KIND_BOLT12_OFFER as u16))
            .limit(1),
    )
    .await?;
    let event = events
        .into_iter()
        .max_by_key(|event| (event.created_at, event.id))
        .ok_or_else(|| protocol_error("Agent has no active BOLT12 offer"))?;
    validate_offer_event(&event, agent)
}

pub async fn validate_instruction(
    terms: &PaidRuntimeTerms,
    rest: &RestClient,
    instruction: &Event,
    channel_id: &str,
) -> anyhow::Result<()> {
    let agent = terms.keys.public_key();
    let payer_hex = instruction.pubkey.to_hex();
    if !terms.priced
        || !payer_has_paid_access(&terms.respond_to, &terms.respond_to_allowlist, &payer_hex)
    {
        return Err(protocol_error("paid Agent access is unavailable"));
    }
    let zap_id = runtime_zap_id(instruction, &agent.to_hex())?;
    let zap_event_id = EventId::from_hex(&zap_id)?;
    let zap = query_events(
        rest,
        nostr::Filter::new()
            .id(zap_event_id)
            .kind(Kind::Custom(KIND_BOLT12_ZAP as u16)),
    )
    .await?
    .into_iter()
    .find(|event| event.id == zap_event_id)
    .ok_or_else(|| protocol_error("access zap is unavailable"))?;
    let pricing_id = EventId::from_hex(exactly_one_tag(&zap, "e")?)?;
    let pricing_event = query_events(
        rest,
        nostr::Filter::new()
            .id(pricing_id)
            .kind(Kind::Custom(KIND_AGENT_RUNTIME_PRICING as u16)),
    )
    .await?
    .into_iter()
    .find(|event| event.id == pricing_id)
    .ok_or_else(|| protocol_error("zap pricing event is unavailable"))?;
    validate_zap_chain(&zap, instruction, &pricing_event, channel_id, agent)
}

#[cfg(test)]
mod tests {
    use super::*;
    use buzz_core::agent_runtime_payment::RuntimePricing;
    use nostr::{EventBuilder, JsonUtil, Tag, Timestamp};

    const VALID_OFFER: &str =
        "lno1pgx9getnwss8vetrw3hhyuckyypwa3eyt44h6txtxquqh7lz5djge4afgfjn7k4rgrkuag0jsd5xvxg";

    fn tag(values: [&str; 2]) -> Tag {
        Tag::parse(values).unwrap()
    }

    #[test]
    fn pricing_uses_a_flat_five_minute_window() {
        let pricing = RuntimePricing::enabled(255).unwrap();
        assert_eq!(pricing.price_sats, Some(255));
        assert_eq!(pricing.invocation_window_seconds, Some(300));
    }

    #[test]
    fn runtime_tag_uses_the_zap_event_id() {
        let keys = nostr::Keys::generate();
        let zap = EventBuilder::text_note("paid")
            .sign_with_keys(&keys)
            .unwrap();
        let instruction = EventBuilder::text_note("run")
            .tag(
                Tag::parse([
                    "agent_runtime",
                    keys.public_key().to_hex().as_str(),
                    zap.id.to_hex().as_str(),
                ])
                .unwrap(),
            )
            .sign_with_keys(&nostr::Keys::generate())
            .unwrap();
        assert_eq!(
            runtime_zap_id(&instruction, &keys.public_key().to_hex()).unwrap(),
            zap.id.to_hex()
        );
    }

    #[test]
    fn multiple_runtime_tags_are_rejected() {
        let agent = nostr::Keys::generate();
        let zap = EventBuilder::text_note("paid")
            .sign_with_keys(&agent)
            .unwrap();
        let runtime_tag = Tag::parse([
            "agent_runtime",
            agent.public_key().to_hex().as_str(),
            zap.id.to_hex().as_str(),
        ])
        .unwrap();
        let instruction = EventBuilder::new(Kind::Custom(KIND_STREAM_MESSAGE as u16), "run")
            .tags([runtime_tag.clone(), runtime_tag])
            .sign_with_keys(&nostr::Keys::generate())
            .unwrap();
        assert!(runtime_zap_id(&instruction, &agent.public_key().to_hex()).is_err());
    }

    #[test]
    fn canonical_offer_validation_rejects_a_withdrawal() {
        let keys = nostr::Keys::generate();
        let withdrawal = EventBuilder::new(Kind::Custom(KIND_BOLT12_OFFER as u16), "")
            .sign_with_keys(&keys)
            .unwrap();
        assert!(validate_offer_event(&withdrawal, keys.public_key()).is_err());
        let malformed = EventBuilder::new(Kind::Custom(KIND_BOLT12_OFFER as u16), "")
            .tag(tag(["offer", VALID_OFFER]))
            .sign_with_keys(&keys)
            .unwrap();
        validate_offer_event(&malformed, keys.public_key()).unwrap();
    }

    fn paid_chain() -> (Event, Event, Event, PublicKey, nostr::Keys, String) {
        let agent = nostr::Keys::generate();
        let payer = nostr::Keys::generate();
        let channel = "9a1657ac-f7aa-5db0-b632-d8bbeb6dfb50".to_string();
        let offer = EventBuilder::new(Kind::Custom(KIND_BOLT12_OFFER as u16), "")
            .tag(tag(["offer", VALID_OFFER]))
            .sign_with_keys(&agent)
            .unwrap();
        let pricing = EventBuilder::new(
            Kind::Custom(KIND_AGENT_RUNTIME_PRICING as u16),
            serde_json::to_string(&RuntimePricing::enabled(255).unwrap()).unwrap(),
        )
        .sign_with_keys(&agent)
        .unwrap();
        let agent_hex = agent.public_key().to_hex();
        let pricing_id = pricing.id.to_hex();
        let offer_json = offer.as_json();
        let intent = EventBuilder::new(Kind::Custom(KIND_BOLT12_ZAP_INTENT as u16), "run")
            .tags([
                Tag::parse(["p", agent_hex.as_str()]).unwrap(),
                Tag::parse(["e", pricing_id.as_str()]).unwrap(),
                Tag::parse(["h", channel.as_str()]).unwrap(),
                tag(["amount", "255000"]),
                Tag::parse(["offer_event", offer_json.as_str()]).unwrap(),
                tag(["zap_id", "00112233445566778899aabbccddeeff"]),
            ])
            .sign_with_keys(&payer)
            .unwrap();
        let mut zap_tags = intent
            .tags
            .iter()
            .filter(|tag| tag.as_slice().first().map(String::as_str) != Some("zap_id"))
            .cloned()
            .collect::<Vec<_>>();
        let intent_json = intent.as_json();
        let payer_hex = payer.public_key().to_hex();
        zap_tags.extend([
            Tag::parse(["description", intent_json.as_str()]).unwrap(),
            Tag::parse(["P", payer_hex.as_str()]).unwrap(),
            tag(["proof", PLACEHOLDER_PAYER_PROOF]),
        ]);
        let zap = EventBuilder::new(Kind::Custom(KIND_BOLT12_ZAP as u16), "run")
            .tags(zap_tags)
            .sign_with_keys(&payer)
            .unwrap();
        let instruction = EventBuilder::new(Kind::Custom(KIND_STREAM_MESSAGE as u16), "run")
            .custom_created_at(zap.created_at)
            .tags([
                Tag::parse(["h", channel.as_str()]).unwrap(),
                Tag::parse([
                    "agent_runtime",
                    agent_hex.as_str(),
                    zap.id.to_hex().as_str(),
                ])
                .unwrap(),
            ])
            .sign_with_keys(&payer)
            .unwrap();
        (
            zap,
            instruction,
            pricing,
            agent.public_key(),
            payer,
            channel,
        )
    }

    #[test]
    fn one_zap_can_start_more_than_one_invocation() {
        let (zap, first, pricing, agent, payer, channel) = paid_chain();
        validate_zap_chain(&zap, &first, &pricing, &channel, agent).unwrap();
        let second = EventBuilder::new(Kind::Custom(KIND_STREAM_MESSAGE as u16), "run again")
            .custom_created_at(zap.created_at)
            .tags(first.tags.clone())
            .sign_with_keys(&payer)
            .unwrap();
        validate_zap_chain(&zap, &second, &pricing, &channel, agent).unwrap();
    }

    #[test]
    fn invocation_after_the_window_is_rejected() {
        let (zap, instruction, pricing, agent, payer, channel) = paid_chain();
        let expired = EventBuilder::new(Kind::Custom(KIND_STREAM_MESSAGE as u16), "late")
            .custom_created_at(Timestamp::from(zap.created_at.as_secs() + 301))
            .tags(instruction.tags.clone())
            .sign_with_keys(&payer)
            .unwrap();
        assert!(validate_zap_chain(&zap, &expired, &pricing, &channel, agent).is_err());
    }

    #[test]
    fn instruction_with_a_different_author_is_rejected() {
        let (zap, instruction, pricing, agent, _, channel) = paid_chain();
        let impostor = EventBuilder::new(Kind::Custom(KIND_STREAM_MESSAGE as u16), "run")
            .custom_created_at(zap.created_at)
            .tags(instruction.tags.clone())
            .sign_with_keys(&nostr::Keys::generate())
            .unwrap();
        assert!(validate_zap_chain(&zap, &impostor, &pricing, &channel, agent).is_err());
    }

    #[test]
    fn instruction_for_a_different_channel_is_rejected() {
        let (zap, instruction, pricing, agent, payer, channel) = paid_chain();
        let wrong_channel = EventBuilder::new(Kind::Custom(KIND_STREAM_MESSAGE as u16), "run")
            .custom_created_at(zap.created_at)
            .tags(
                instruction
                    .tags
                    .iter()
                    .filter(|tag| tag.as_slice().first().map(String::as_str) != Some("h"))
                    .cloned()
                    .chain([tag(["h", "4cdf511d-caf5-49a6-9217-61c72c38e6de"])])
                    .collect::<Vec<_>>(),
            )
            .sign_with_keys(&payer)
            .unwrap();
        assert!(validate_zap_chain(&zap, &wrong_channel, &pricing, &channel, agent).is_err());
    }

    #[test]
    fn instruction_with_multiple_channel_tags_is_rejected() {
        let (zap, instruction, pricing, agent, payer, channel) = paid_chain();
        let mut tags = instruction.tags.to_vec();
        tags.push(tag(["h", channel.as_str()]));
        let duplicate = EventBuilder::new(Kind::Custom(KIND_STREAM_MESSAGE as u16), "run")
            .custom_created_at(zap.created_at)
            .tags(tags)
            .sign_with_keys(&payer)
            .unwrap();
        assert!(validate_zap_chain(&zap, &duplicate, &pricing, &channel, agent).is_err());
    }

    #[test]
    fn non_message_instruction_is_rejected() {
        let (zap, instruction, pricing, agent, payer, channel) = paid_chain();
        let wrong_kind = EventBuilder::text_note("run")
            .custom_created_at(zap.created_at)
            .tags(instruction.tags.clone())
            .sign_with_keys(&payer)
            .unwrap();
        assert!(validate_zap_chain(&zap, &wrong_kind, &pricing, &channel, agent).is_err());
    }

    #[test]
    fn invalid_instruction_signature_is_rejected() {
        let (zap, mut instruction, pricing, agent, _, channel) = paid_chain();
        instruction.content.push_str(" tampered");
        assert!(validate_zap_chain(&zap, &instruction, &pricing, &channel, agent).is_err());
    }

    #[test]
    fn zap_with_a_different_payer_tag_is_rejected() {
        let (zap, instruction, pricing, agent, payer, channel) = paid_chain();
        let mut tags = zap
            .tags
            .iter()
            .filter(|tag| tag.as_slice().first().map(String::as_str) != Some("P"))
            .cloned()
            .collect::<Vec<_>>();
        tags.push(tag([
            "P",
            nostr::Keys::generate().public_key().to_hex().as_str(),
        ]));
        let wrong_payer = EventBuilder::new(Kind::Custom(KIND_BOLT12_ZAP as u16), "run")
            .tags(tags)
            .sign_with_keys(&payer)
            .unwrap();
        assert!(validate_zap_chain(&wrong_payer, &instruction, &pricing, &channel, agent).is_err());
    }

    #[test]
    fn zap_for_a_different_agent_is_rejected() {
        let (zap, instruction, pricing, _, _, channel) = paid_chain();
        assert!(validate_zap_chain(
            &zap,
            &instruction,
            &pricing,
            &channel,
            nostr::Keys::generate().public_key(),
        )
        .is_err());
    }

    #[test]
    fn zap_for_a_different_pricing_event_is_rejected() {
        let (zap, instruction, _, agent, _, channel) = paid_chain();
        let different_pricing = EventBuilder::new(
            Kind::Custom(KIND_AGENT_RUNTIME_PRICING as u16),
            serde_json::to_string(&RuntimePricing::enabled(255).unwrap()).unwrap(),
        )
        .sign_with_keys(&nostr::Keys::generate())
        .unwrap();
        assert!(
            validate_zap_chain(&zap, &instruction, &different_pricing, &channel, agent).is_err()
        );
    }
}

//! Validation for settled BOLT12 zap events.

use std::str::FromStr;

use lightning::offers::{offer::Offer, payer_proof::PayerProof};
use nostr::{Event, JsonUtil, Kind};
use thiserror::Error;

use buzz_core::kind::{KIND_BOLT12_OFFER, KIND_BOLT12_ZAP, KIND_BOLT12_ZAP_INTENT};

/// Temporary proof value used until wallet providers expose payer proofs.
pub const PLACEHOLDER_PAYER_PROOF: &str = "placeholder";

/// A settled zap after the complete signed proof chain passes validation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ValidatedBolt12Zap {
    /// Settled zap event ID.
    pub event_id: String,
    /// Whole satoshis paid to the recipient.
    pub amount: u64,
    /// Comment from the signed zap intent.
    pub comment: String,
    /// Signed zap intent event ID.
    pub intent_event_id: String,
    /// Recipient public key from the signed proof chain.
    pub recipient_pubkey: String,
    /// Target event ID for an event zap.
    pub target_event_id: Option<String>,
    /// Channel ID for a channel-scoped zap.
    pub channel_id: Option<String>,
}

/// A reason that a settled BOLT12 zap is invalid.
#[derive(Clone, Debug, Eq, Error, PartialEq)]
#[error("{message}")]
pub struct Bolt12ZapError {
    message: String,
}

impl Bolt12ZapError {
    fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

fn exact_tag<'a>(event: &'a Event, name: &str) -> Result<&'a str, Bolt12ZapError> {
    let matching = event
        .tags
        .iter()
        .filter(|tag| tag.as_slice().first().map(String::as_str) == Some(name))
        .collect::<Vec<_>>();
    if matching.len() != 1 {
        return Err(Bolt12ZapError::new(format!(
            "zap must contain exactly one {name} tag"
        )));
    }
    matching[0]
        .as_slice()
        .get(1)
        .map(String::as_str)
        .ok_or_else(|| Bolt12ZapError::new(format!("zap {name} tag has no value")))
}

fn optional_tag<'a>(event: &'a Event, name: &str) -> Result<Option<&'a str>, Bolt12ZapError> {
    let matching = event
        .tags
        .iter()
        .filter(|tag| tag.as_slice().first().map(String::as_str) == Some(name))
        .collect::<Vec<_>>();
    if matching.len() > 1 {
        return Err(Bolt12ZapError::new(format!(
            "zap must not contain duplicate {name} tags"
        )));
    }
    matching
        .first()
        .map(|tag| {
            tag.as_slice()
                .get(1)
                .map(String::as_str)
                .ok_or_else(|| Bolt12ZapError::new(format!("zap {name} tag has no value")))
        })
        .transpose()
}

fn require_matching_tag(outer: &Event, intent: &Event, name: &str) -> Result<(), Bolt12ZapError> {
    if optional_tag(outer, name)? != optional_tag(intent, name)? {
        return Err(Bolt12ZapError::new(format!(
            "zap {name} tag does not match its intent"
        )));
    }
    Ok(())
}

fn validate_offer_event(event: &Event, recipient: &str) -> Result<(), Bolt12ZapError> {
    event
        .verify()
        .map_err(|error| Bolt12ZapError::new(format!("invalid offer event: {error}")))?;
    if event.kind != Kind::Custom(KIND_BOLT12_OFFER as u16) || event.pubkey.to_hex() != recipient {
        return Err(Bolt12ZapError::new(
            "offer announcement does not match the recipient",
        ));
    }

    let offers = event
        .tags
        .iter()
        .filter_map(|tag| {
            let parts = tag.as_slice();
            (parts.first().map(String::as_str) == Some("offer"))
                .then(|| parts.get(1).map(String::as_str))
                .flatten()
        })
        .collect::<Vec<_>>();
    if offers.is_empty() || offers.iter().any(|offer| !is_canonical_offer(offer)) {
        return Err(Bolt12ZapError::new(
            "offer announcement must contain only canonical BOLT12 offers",
        ));
    }
    Ok(())
}

fn is_canonical_offer(value: &str) -> bool {
    value.starts_with("lno1")
        && !value.chars().any(char::is_whitespace)
        && value == value.to_ascii_lowercase()
        && Offer::from_str(value).is_ok_and(|offer| offer.to_string() == value)
}

/// Validate a settled BOLT12 zap and its embedded signed proof chain.
pub fn validate_bolt12_zap(event: &Event) -> Result<ValidatedBolt12Zap, Bolt12ZapError> {
    event
        .verify()
        .map_err(|error| Bolt12ZapError::new(format!("invalid zap event: {error}")))?;
    if event.kind != Kind::Custom(KIND_BOLT12_ZAP as u16) {
        return Err(Bolt12ZapError::new("event is not a BOLT12 zap"));
    }

    let recipient = exact_tag(event, "p")?.to_ascii_lowercase();
    let amount_text = exact_tag(event, "amount")?;
    let description = exact_tag(event, "description")?;
    let offer_event_json = exact_tag(event, "offer_event")?;
    let proof = exact_tag(event, "proof")?;
    let payer_proof = (proof != PLACEHOLDER_PAYER_PROOF)
        .then(|| {
            PayerProof::from_str(proof)
                .map_err(|error| Bolt12ZapError::new(format!("invalid payer proof: {error:?}")))
        })
        .transpose()?;

    let amount_msats = amount_text
        .parse::<u64>()
        .map_err(|_| Bolt12ZapError::new("zap amount is not an integer"))?;
    if amount_msats == 0 || !amount_msats.is_multiple_of(1_000) {
        return Err(Bolt12ZapError::new(
            "zap amount must be a positive whole-satoshi value",
        ));
    }
    if payer_proof
        .as_ref()
        .and_then(PayerProof::invoice_amount_msats)
        .is_some_and(|proof_amount| proof_amount != amount_msats)
    {
        return Err(Bolt12ZapError::new(
            "zap amount does not match its payer proof",
        ));
    }

    let intent = Event::from_json(description)
        .map_err(|error| Bolt12ZapError::new(format!("invalid zap intent: {error}")))?;
    intent
        .verify()
        .map_err(|error| Bolt12ZapError::new(format!("invalid zap intent: {error}")))?;
    let offer_event = Event::from_json(offer_event_json)
        .map_err(|error| Bolt12ZapError::new(format!("invalid offer event: {error}")))?;

    if intent.kind != Kind::Custom(KIND_BOLT12_ZAP_INTENT as u16)
        || intent.pubkey != event.pubkey
        || intent.content != event.content
        || exact_tag(&intent, "p")?.to_ascii_lowercase() != recipient
        || exact_tag(&intent, "amount")? != amount_text
        || exact_tag(&intent, "offer_event")? != offer_event_json
    {
        return Err(Bolt12ZapError::new("zap does not match its signed intent"));
    }

    let zap_id = exact_tag(&intent, "zap_id")?;
    if zap_id.len() < 32
        || !zap_id.len().is_multiple_of(2)
        || !zap_id
            .chars()
            .all(|character| character.is_ascii_hexdigit() && !character.is_ascii_uppercase())
    {
        return Err(Bolt12ZapError::new("zap intent has an invalid zap_id"));
    }

    for name in ["e", "a", "k"] {
        require_matching_tag(event, &intent, name)?;
    }
    if optional_tag(&intent, "h")?.is_some() {
        require_matching_tag(event, &intent, "h")?;
    }
    let target_event_id = optional_tag(event, "e")?.map(str::to_string);
    let channel_id = optional_tag(event, "h")?.map(str::to_string);
    if target_event_id.is_some() && optional_tag(event, "a")?.is_some() {
        return Err(Bolt12ZapError::new(
            "zap cannot target both an event and an address",
        ));
    }

    validate_offer_event(&offer_event, &recipient)?;
    if offer_event.created_at > intent.created_at {
        return Err(Bolt12ZapError::new(
            "zap intent predates its offer announcement",
        ));
    }
    if optional_tag(event, "P")?
        .is_some_and(|payer| payer.to_ascii_lowercase() != event.pubkey.to_hex())
    {
        return Err(Bolt12ZapError::new(
            "zap payer tag does not match its signer",
        ));
    }

    Ok(ValidatedBolt12Zap {
        event_id: event.id.to_hex(),
        amount: amount_msats / 1_000,
        comment: intent.content,
        intent_event_id: intent.id.to_hex(),
        recipient_pubkey: recipient,
        target_event_id,
        channel_id,
    })
}

#[cfg(test)]
mod tests {
    use nostr::{EventBuilder, Keys, Tag};

    use super::*;

    const VALID_OFFER: &str =
        "lno1pgx9getnwss8vetrw3hhyuckyypwa3eyt44h6txtxquqh7lz5djge4afgfjn7k4rgrkuag0jsd5xvxg";

    fn tagged_zap(offer: &str, proof: &str) -> (Event, Event) {
        let payer = Keys::generate();
        let recipient = Keys::generate();
        let offer_event = EventBuilder::new(Kind::Custom(KIND_BOLT12_OFFER as u16), "")
            .tag(Tag::parse(["offer", offer]).unwrap())
            .sign_with_keys(&recipient)
            .unwrap();
        let target = "ab".repeat(32);
        let intent = EventBuilder::new(Kind::Custom(KIND_BOLT12_ZAP_INTENT as u16), "nice work")
            .tags([
                Tag::parse(["p", recipient.public_key().to_hex().as_str()]).unwrap(),
                Tag::parse(["amount", "21000"]).unwrap(),
                Tag::parse(["offer_event", offer_event.as_json().as_str()]).unwrap(),
                Tag::parse(["zap_id", "00112233445566778899aabbccddeeff"]).unwrap(),
                Tag::parse(["e", target.as_str()]).unwrap(),
                Tag::parse(["k", "40002"]).unwrap(),
            ])
            .sign_with_keys(&payer)
            .unwrap();
        let mut tags = intent
            .tags
            .iter()
            .filter(|tag| tag.as_slice().first().map(String::as_str) != Some("zap_id"))
            .cloned()
            .collect::<Vec<_>>();
        tags.extend([
            Tag::parse(["description", intent.as_json().as_str()]).unwrap(),
            Tag::parse(["P", payer.public_key().to_hex().as_str()]).unwrap(),
            Tag::parse(["proof", proof]).unwrap(),
        ]);
        let zap = EventBuilder::new(Kind::Custom(KIND_BOLT12_ZAP as u16), "nice work")
            .tags(tags)
            .sign_with_keys(&payer)
            .unwrap();
        (intent, zap)
    }

    #[test]
    fn validates_complete_placeholder_proof_chain() {
        let (intent, zap) = tagged_zap(VALID_OFFER, PLACEHOLDER_PAYER_PROOF);
        let parsed = validate_bolt12_zap(&zap).unwrap();
        assert_eq!(parsed.event_id, zap.id.to_hex());
        assert_eq!(parsed.intent_event_id, intent.id.to_hex());
        assert_eq!(parsed.amount, 21);
        assert_eq!(parsed.comment, "nice work");
        assert_eq!(parsed.target_event_id, Some("ab".repeat(32)));
    }

    #[test]
    fn rejects_noncanonical_offer() {
        let (_, zap) = tagged_zap(
            &VALID_OFFER[..VALID_OFFER.len() - 1],
            PLACEHOLDER_PAYER_PROOF,
        );
        assert!(validate_bolt12_zap(&zap).is_err());
    }

    #[test]
    fn rejects_invalid_payer_proof() {
        let (_, zap) = tagged_zap(VALID_OFFER, "lnp1qqqq");
        assert!(validate_bolt12_zap(&zap).is_err());
    }
}

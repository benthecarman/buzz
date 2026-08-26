//! Validation for settled BOLT12 zap events.

use std::str::FromStr;

use lightning_payer_proof::{verify, Offer};
use nostr::{Event, JsonUtil, Kind, Tag};
use thiserror::Error;

use buzz_core::kind::{KIND_BOLT12_OFFER, KIND_BOLT12_ZAP, KIND_BOLT12_ZAP_INTENT};

/// A settled zap after the complete signed proof chain passes validation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ValidatedBolt12Zap {
    /// Settled zap event ID.
    pub event_id: String,
    /// Payment hash that uniquely identifies the settled Lightning payment.
    pub payment_hash: [u8; 32],
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

fn validate_offer_event(event: &Event, recipient: &str) -> Result<Vec<Offer>, Bolt12ZapError> {
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
        .filter_map(|value| {
            let offer = Offer::from_str(value).ok()?;
            (offer.to_string() == value).then_some(offer)
        })
        .collect::<Vec<_>>();
    if offers.is_empty() {
        return Err(Bolt12ZapError::new(
            "offer announcement must contain a canonical BOLT12 offer",
        ));
    }
    Ok(offers)
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
    if !proof.starts_with("lnp1")
        || proof != proof.to_ascii_lowercase()
        || proof.chars().any(char::is_whitespace)
        || proof.contains('+')
    {
        return Err(Bolt12ZapError::new(
            "payer proof is not canonically encoded",
        ));
    }
    let payer_proof = verify(proof)
        .map_err(|error| Bolt12ZapError::new(format!("invalid payer proof: {error}")))?;

    let amount_msats = amount_text
        .parse::<u64>()
        .map_err(|_| Bolt12ZapError::new("zap amount is not an integer"))?;
    if amount_msats == 0 || !amount_msats.is_multiple_of(1_000) {
        return Err(Bolt12ZapError::new(
            "zap amount must be a positive whole-satoshi value",
        ));
    }
    if payer_proof.invoice_amount_msats() != Some(amount_msats) {
        return Err(Bolt12ZapError::new(
            "zap amount is missing from or does not match its payer proof",
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
    let expected_proof_note = format!("nostr:nipB1:{}", intent.id.to_hex());
    if payer_proof.proof_note().map(|note| note.0).as_deref() != Some(expected_proof_note.as_str())
    {
        return Err(Bolt12ZapError::new(
            "payer proof does not name the signed zap intent",
        ));
    }
    let payment_hash = payer_proof.payment_hash().0;
    let mut expected_tags = intent
        .tags
        .iter()
        .filter(|tag| tag.as_slice().first().map(String::as_str) != Some("zap_id"))
        .cloned()
        .collect::<Vec<_>>();
    expected_tags.extend([
        Tag::parse(["description", intent.as_json().as_str()])
            .map_err(|_| Bolt12ZapError::new("invalid canonical description tag"))?,
        Tag::parse(["P", event.pubkey.to_hex().as_str()])
            .map_err(|_| Bolt12ZapError::new("invalid canonical payer tag"))?,
        Tag::parse(["proof", proof])
            .map_err(|_| Bolt12ZapError::new("invalid canonical proof tag"))?,
    ]);
    if !event.tags.iter().eq(expected_tags.iter()) {
        return Err(Bolt12ZapError::new(
            "zap is not the canonical envelope for its intent and proof",
        ));
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

    let offers = validate_offer_event(&offer_event, &recipient)?;
    if !offers
        .iter()
        .any(|offer| payer_proof.pays_offers_recipient(offer))
    {
        return Err(Bolt12ZapError::new(
            "payer proof does not pay an announced offer recipient",
        ));
    }
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
        payment_hash,
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
    use nostr::{EventBuilder, Keys, Tag, Timestamp};

    use super::*;
    use buzz_core::payer_proof_test_utils::payer_proof_for_note;

    const OTHER_OFFER_HEX: &str = "0802a4100a0c636f66666565206265616e731621035be5e9478209674a96e60f1f037f6176540fd001fa1d64694770c56a7709c42c";

    fn decode_hex(value: &str) -> Vec<u8> {
        hex::decode(value).unwrap()
    }

    fn offer_from_hex(value: &str) -> String {
        Offer::try_from(decode_hex(value)).unwrap().to_string()
    }

    fn valid_offer() -> String {
        payer_proof_for_note("").0
    }

    fn payer_keys() -> Keys {
        Keys::parse("0101010101010101010101010101010101010101010101010101010101010101").unwrap()
    }

    fn tagged_zap_with_offers(offers: &[&str], proof: Option<&str>) -> (Event, Event) {
        let payer = payer_keys();
        let recipient = Keys::generate();
        let offer_event = EventBuilder::new(Kind::Custom(KIND_BOLT12_OFFER as u16), "")
            .tags(
                offers
                    .iter()
                    .map(|offer| Tag::parse(["offer", *offer]).unwrap()),
            )
            .sign_with_keys(&recipient)
            .unwrap();
        let target = "ab".repeat(32);
        let intent = EventBuilder::new(Kind::Custom(KIND_BOLT12_ZAP_INTENT as u16), "nice work")
            .tags([
                Tag::parse(["p", recipient.public_key().to_hex().as_str()]).unwrap(),
                Tag::parse(["amount", "42000"]).unwrap(),
                Tag::parse(["offer_event", offer_event.as_json().as_str()]).unwrap(),
                Tag::parse(["zap_id", "00112233445566778899aabbccddeeff"]).unwrap(),
                Tag::parse(["e", target.as_str()]).unwrap(),
                Tag::parse(["k", "40002"]).unwrap(),
            ])
            .sign_with_keys(&payer)
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

    fn tagged_zap(offer: &str, proof: Option<&str>) -> (Event, Event) {
        tagged_zap_with_offers(&[offer], proof)
    }

    #[test]
    fn validates_complete_payer_proof_chain() {
        let offer = valid_offer();
        let (intent, zap) = tagged_zap(&offer, None);
        let parsed = validate_bolt12_zap(&zap).unwrap();
        assert_eq!(parsed.event_id, zap.id.to_hex());
        assert_eq!(
            parsed.payment_hash,
            verify(exact_tag(&zap, "proof").unwrap())
                .unwrap()
                .payment_hash()
                .0
        );
        assert_eq!(parsed.intent_event_id, intent.id.to_hex());
        assert_eq!(parsed.amount, 42);
        assert_eq!(parsed.comment, "nice work");
        assert_eq!(parsed.target_event_id, Some("ab".repeat(32)));
    }

    #[test]
    fn skips_invalid_offer_tags() {
        let offer = valid_offer();
        let (_, zap) = tagged_zap_with_offers(&["invalid-offer", &offer], None);
        assert!(validate_bolt12_zap(&zap).is_ok());
    }

    #[test]
    fn rejects_noncanonical_offer() {
        let offer = valid_offer();
        let (_, zap) = tagged_zap(&offer[..offer.len() - 1], None);
        assert!(validate_bolt12_zap(&zap).is_err());
    }

    #[test]
    fn rejects_invalid_payer_proof() {
        let offer = valid_offer();
        let (_, zap) = tagged_zap(&offer, Some("lnp1qqqq"));
        assert!(validate_bolt12_zap(&zap).is_err());
    }

    #[test]
    fn rejects_proof_bound_to_another_intent() {
        let offer = valid_offer();
        let (_, wrong_proof, _) = payer_proof_for_note(&format!("nostr:nipB1:{}", "00".repeat(32)));
        let (_, zap) = tagged_zap(&offer, Some(&wrong_proof));
        assert!(validate_bolt12_zap(&zap).is_err());
    }

    #[test]
    fn accepts_an_outer_timestamp_independent_of_the_invoice() {
        let offer = valid_offer();
        let (_, zap) = tagged_zap(&offer, None);
        let original = validate_bolt12_zap(&zap).unwrap();
        let altered = EventBuilder::new(zap.kind, zap.content.clone())
            .tags(zap.tags.iter().cloned())
            .custom_created_at(Timestamp::from(zap.created_at.as_secs() + 1))
            .sign_with_keys(&payer_keys())
            .unwrap();
        let altered = validate_bolt12_zap(&altered).unwrap();

        assert_ne!(altered.event_id, original.event_id);
        assert_eq!(altered.payment_hash, original.payment_hash);
    }

    #[test]
    fn rejects_extra_outer_tags() {
        let offer = valid_offer();
        let (_, zap) = tagged_zap(&offer, None);
        let mut tags = zap.tags.iter().cloned().collect::<Vec<_>>();
        tags.push(Tag::parse(["client", "alternate-wrapper"]).unwrap());
        let altered = EventBuilder::new(zap.kind, zap.content.clone())
            .tags(tags)
            .custom_created_at(zap.created_at)
            .sign_with_keys(&payer_keys())
            .unwrap();

        assert!(validate_bolt12_zap(&altered).is_err());
    }

    #[test]
    fn rejects_proof_for_another_offer_recipient() {
        let (_, zap) = tagged_zap(&offer_from_hex(OTHER_OFFER_HEX), None);
        assert!(validate_bolt12_zap(&zap).is_err());
    }
}

use super::*;

fn random_zap_id() -> Result<String, WalletError> {
    let mut bytes = [0_u8; 16];
    getrandom::fill(&mut bytes)
        .map_err(|error| WalletError::unavailable(format!("generate zap id: {error}")))?;
    Ok(hex::encode(bytes))
}

/// Event and channel fields bound into one zap intent.
pub struct ZapTarget {
    pub event_id: Option<String>,
    pub event_kind: Option<u32>,
    pub channel_id: Option<String>,
    pub lease_id: Option<String>,
}

pub(super) fn signed_intent(
    keys: &Keys,
    recipient: &WalletRecipientOffer,
    amount: u64,
    comment: &str,
    target: &ZapTarget,
) -> Result<Event, WalletError> {
    if target.event_id.is_some() != target.event_kind.is_some() {
        return Err(WalletError::new(
            "invalid_zap",
            "event zaps require both a target event id and kind",
        ));
    }
    if let Some(event_id) = target.event_id.as_deref() {
        if event_id.len() != 64
            || !event_id
                .chars()
                .all(|character| character.is_ascii_hexdigit())
            || event_id != event_id.to_ascii_lowercase()
        {
            return Err(WalletError::new(
                "invalid_zap",
                "zap target event id must be 32-byte lowercase hex",
            ));
        }
    }

    let mut tags = vec![
        tag(["p", recipient.recipient_pubkey.as_str()])?,
        tag(["amount", amount_msats(amount)?.as_str()])?,
        tag(["offer_event", recipient.offer_event_json.as_str()])?,
        tag(["zap_id", random_zap_id()?.as_str()])?,
    ];
    if let (Some(event_id), Some(event_kind)) = (target.event_id.as_deref(), target.event_kind) {
        tags.push(tag(["e", event_id])?);
        let event_kind = event_kind.to_string();
        tags.push(tag(["k", event_kind.as_str()])?);
    }
    if let Some(channel_id) = target.channel_id.as_deref() {
        tags.push(tag(["h", channel_id])?);
    }
    if let Some(lease_id) = target.lease_id.as_deref() {
        let lease_id = Uuid::parse_str(lease_id)
            .map_err(|_| WalletError::new("invalid_zap", "lease id must be a UUID"))?
            .to_string();
        tags.push(tag(["lease", lease_id.as_str()])?);
    }
    let intent = EventBuilder::new(Kind::Custom(KIND_BOLT12_ZAP_INTENT as u16), comment)
        .tags(tags)
        .sign_with_keys(keys)
        .map_err(|error| WalletError::new("invalid_zap", format!("sign zap intent: {error}")))?;
    let offer_event = Event::from_json(&recipient.offer_event_json)
        .map_err(|error| WalletError::new("offer_invalid", error.to_string()))?;
    if offer_event.created_at > intent.created_at {
        return Err(WalletError::new(
            "offer_invalid",
            "offer announcement is newer than the zap intent",
        ));
    }
    Ok(intent)
}

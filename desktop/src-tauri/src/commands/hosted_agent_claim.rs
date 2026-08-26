use nostr::{Event, EventId, JsonUtil, Keys, Kind, PublicKey};

pub(super) struct ValidatedHostedAgentClaim {
    pub agent_pubkey: String,
    pub agent_name: String,
}

fn one_tag(event: &Event, name: &str) -> Result<Vec<String>, String> {
    let tags = event
        .tags
        .iter()
        .filter(|tag| tag.as_slice().first().map(String::as_str) == Some(name))
        .map(|tag| tag.as_slice().to_vec())
        .collect::<Vec<_>>();
    if tags.len() != 1 {
        return Err(format!("ownership request must contain one {name} tag"));
    }
    Ok(tags.into_iter().next().unwrap_or_default())
}

fn marked_event(event: &Event, marker: &str) -> Result<String, String> {
    let values = event
        .tags
        .iter()
        .filter_map(|tag| {
            let values = tag.as_slice();
            (values.first().map(String::as_str) == Some("e")
                && values.get(3).map(String::as_str) == Some(marker))
            .then(|| values.get(1).cloned())
            .flatten()
        })
        .collect::<Vec<_>>();
    if values.len() != 1 {
        return Err(format!(
            "ownership request must contain one {marker} reference"
        ));
    }
    Ok(values.into_iter().next().unwrap_or_default())
}

fn validate_derived_identity(
    factory_pubkey: &PublicKey,
    buyer_keys: &Keys,
    plan_event_id: &EventId,
    intent_event_id: &str,
    requested_agent: &PublicKey,
    requested_lease: &str,
) -> Result<(), String> {
    let intent_event_id = EventId::from_hex(intent_event_id)
        .map_err(|_| "purchase has an invalid intent event ID")?;
    let expected_agent = buzz_core_pkg::hosted_agent::derive_hosted_agent_keys(
        buyer_keys.secret_key(),
        factory_pubkey,
        factory_pubkey,
        &buyer_keys.public_key(),
        plan_event_id,
        &intent_event_id,
    )
    .map_err(|error| format!("derive hosted-agent identity: {error}"))?;
    if *requested_agent != expected_agent.public_key() {
        return Err("factory requested ownership of the wrong agent key".into());
    }
    let expected_lease =
        buzz_core_pkg::hosted_agent::derive_hosted_agent_lease_id(requested_agent).to_string();
    if requested_lease != expected_lease {
        return Err("factory requested an invalid lease ID".into());
    }
    Ok(())
}

pub(super) fn validate(
    request_json: &str,
    zap_json: &str,
    plan_json: &str,
    buyer_keys: &Keys,
) -> Result<ValidatedHostedAgentClaim, String> {
    let request = Event::from_json(request_json)
        .map_err(|error| format!("invalid ownership request: {error}"))?;
    request
        .verify()
        .map_err(|error| format!("invalid ownership request signature: {error}"))?;
    if request.kind != Kind::Custom(40002) {
        return Err("event is not a hosted-agent claim request".into());
    }
    let zap =
        Event::from_json(zap_json).map_err(|error| format!("invalid purchase zap: {error}"))?;
    let plan =
        Event::from_json(plan_json).map_err(|error| format!("invalid agent plan: {error}"))?;
    let purchase =
        buzz_core_pkg::hosted_agent::validate_purchase_zap(&zap, &plan, &request.pubkey.to_hex())
            .map_err(|error| format!("invalid hosted-agent purchase: {error}"))?;
    if purchase.lease_id.is_some() {
        return Err("a renewal zap cannot create another hosted agent".into());
    }
    if purchase.payer_pubkey != buyer_keys.public_key().to_hex() {
        return Err("ownership request is for a different buyer".into());
    }
    if marked_event(&request, "zap")? != zap.id.to_hex()
        || marked_event(&request, "plan")? != plan.id.to_hex()
    {
        return Err("ownership request does not match its paid purchase".into());
    }
    let lease = one_tag(&request, "d")?
        .get(1)
        .cloned()
        .ok_or_else(|| "ownership request has no lease ID".to_string())?;
    uuid::Uuid::parse_str(&lease).map_err(|_| "ownership request has an invalid lease ID")?;
    let channel = one_tag(&request, "h")?
        .get(1)
        .cloned()
        .ok_or_else(|| "ownership request has no DM channel".to_string())?;
    uuid::Uuid::parse_str(&channel).map_err(|_| "ownership request has an invalid DM channel")?;
    let buyer = one_tag(&request, "p")?
        .get(1)
        .cloned()
        .ok_or_else(|| "ownership request has no buyer".to_string())?;
    if buyer != buyer_keys.public_key().to_hex() {
        return Err("ownership request is for a different buyer".into());
    }
    let agent_pubkey = one_tag(&request, "agent")?
        .get(1)
        .cloned()
        .ok_or_else(|| "ownership request has no agent".to_string())?;
    let requested_agent = PublicKey::from_hex(&agent_pubkey)
        .map_err(|_| "ownership request has an invalid agent pubkey")?;
    validate_derived_identity(
        &request.pubkey,
        buyer_keys,
        &plan.id,
        &purchase.intent_event_id,
        &requested_agent,
        &lease,
    )?;
    let agent_name = one_tag(&request, "name")?
        .get(1)
        .cloned()
        .ok_or_else(|| "ownership request has no agent name".to_string())?;
    if agent_name.trim().is_empty() || agent_name.len() > 128 {
        return Err("ownership request has an invalid agent name".into());
    }
    Ok(ValidatedHostedAgentClaim {
        agent_pubkey,
        agent_name,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use nostr::SecretKey;

    #[test]
    fn factory_claim_must_name_the_derived_identity() {
        let buyer = Keys::new(SecretKey::from_slice(&[3; 32]).unwrap());
        let factory = Keys::new(SecretKey::from_slice(&[4; 32]).unwrap());
        let plan = EventId::from_slice(&[5; 32]).unwrap();
        let intent = EventId::from_slice(&[6; 32]).unwrap();
        let agent = buzz_core_pkg::hosted_agent::derive_hosted_agent_keys(
            buyer.secret_key(),
            &factory.public_key(),
            &factory.public_key(),
            &buyer.public_key(),
            &plan,
            &intent,
        )
        .unwrap();
        let lease = buzz_core_pkg::hosted_agent::derive_hosted_agent_lease_id(&agent.public_key())
            .to_string();

        validate_derived_identity(
            &factory.public_key(),
            &buyer,
            &plan,
            &intent.to_hex(),
            &agent.public_key(),
            &lease,
        )
        .unwrap();
        assert!(validate_derived_identity(
            &factory.public_key(),
            &buyer,
            &plan,
            &intent.to_hex(),
            &Keys::generate().public_key(),
            &lease,
        )
        .is_err());
        assert!(validate_derived_identity(
            &factory.public_key(),
            &buyer,
            &plan,
            &intent.to_hex(),
            &agent.public_key(),
            &uuid::Uuid::new_v4().to_string(),
        )
        .is_err());
    }
}

//! Agent-facing NWC wallet requests.

use std::{
    str::FromStr,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use buzz_core::{
    kind::{
        KIND_BOLT12_OFFER, KIND_BOLT12_ZAP, KIND_BOLT12_ZAP_INTENT, KIND_NWC_INFO,
        KIND_NWC_RESPONSE,
    },
    nwc::{build_pay_request, decrypt_pay_response, NwcPayParams, NwcPayResult},
};
use buzz_ws_client::{NostrWsConnection, RelayMessage};
use lexe_api_core::types::offer::Offer as LexeOffer;
use lexe_payment_uri_core::Bip321Uri;
use lightning::offers::offer::Offer;
use nostr::{Event, EventBuilder, EventId, JsonUtil, Kind, PublicKey, Tag};
use serde_json::json;

use crate::{client::BuzzClient, error::CliError, WalletCmd};

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or_default()
}

fn tag(parts: impl IntoIterator<Item = impl Into<String>>) -> Result<Tag, CliError> {
    Tag::parse(parts).map_err(|error| CliError::Other(format!("invalid zap tag: {error}")))
}

fn build_zap_proof(
    client: &BuzzClient,
    intent: &Event,
    result: &NwcPayResult,
    channel_id: Option<&str>,
) -> Result<Event, CliError> {
    if result.state != "settled" {
        return Err(CliError::Other(format!(
            "wallet returned non-settled zap state: {}",
            result.state
        )));
    }
    let proof = result.payer_proof.as_deref().unwrap_or("placeholder");
    let intent_json = intent.as_json();
    let payer_pubkey = client.keys().public_key().to_hex();
    let mut tags = intent
        .tags
        .iter()
        .filter(|tag| tag.as_slice().first().map(String::as_str) != Some("zap_id"))
        .cloned()
        .collect::<Vec<_>>();
    tags.push(tag(["description", intent_json.as_str()])?);
    tags.push(tag(["P", payer_pubkey.as_str()])?);
    tags.push(tag(["proof", proof])?);
    if let Some(channel_id) = channel_id {
        tags.push(tag(["h", channel_id])?);
    }
    EventBuilder::new(Kind::Custom(KIND_BOLT12_ZAP as u16), &intent.content)
        .tags(tags)
        .sign_with_keys(client.keys())
        .map_err(|error| CliError::Other(format!("failed to sign zap proof: {error}")))
}

async fn target_channel_id(
    client: &BuzzClient,
    target: Option<(&str, u32)>,
) -> Result<Option<String>, CliError> {
    let Some((event_id, event_kind)) = target else {
        return Ok(None);
    };
    let raw = client
        .query(&json!({
            "ids": [event_id],
            "kinds": [event_kind],
            "limit": 1
        }))
        .await?;
    let values: Vec<serde_json::Value> = serde_json::from_str(&raw)
        .map_err(|error| CliError::Other(format!("invalid zap target response: {error}")))?;
    Ok(values.into_iter().find_map(|value| {
        Event::from_json(value.to_string())
            .ok()
            .filter(|event| event.verify().is_ok() && event.id.to_hex() == event_id)
            .and_then(|event| {
                event.tags.iter().find_map(|tag| {
                    let parts = tag.as_slice();
                    (parts.first().map(String::as_str) == Some("h"))
                        .then(|| parts.get(1).cloned())
                        .flatten()
                })
            })
    }))
}

fn ws_url(http_url: &str) -> String {
    http_url
        .replacen("https://", "wss://", 1)
        .replacen("http://", "ws://", 1)
}

async fn latest_offer(client: &BuzzClient, recipient: &str) -> Result<(Event, Offer), CliError> {
    let raw = client
        .query(&json!({
            "kinds": [KIND_BOLT12_OFFER],
            "authors": [recipient],
            "limit": 1
        }))
        .await?;
    let values: Vec<serde_json::Value> = serde_json::from_str(&raw)
        .map_err(|error| CliError::Other(format!("invalid offer query response: {error}")))?;
    let value = values
        .first()
        .ok_or_else(|| CliError::Other("recipient has no BOLT12 offer announcement".into()))?;
    let event = Event::from_json(value.to_string())
        .map_err(|error| CliError::Other(format!("invalid offer event: {error}")))?;
    event
        .verify()
        .map_err(|error| CliError::Other(format!("invalid offer signature: {error}")))?;
    if event.pubkey.to_hex() != recipient || event.kind != Kind::Custom(KIND_BOLT12_OFFER as u16) {
        return Err(CliError::Other(
            "offer announcement does not match the recipient".into(),
        ));
    }
    let offers = event
        .tags
        .iter()
        .filter_map(|tag| {
            let parts = tag.as_slice();
            (parts.first().map(String::as_str) == Some("offer"))
                .then(|| parts.get(1).cloned())
                .flatten()
        })
        .collect::<Vec<_>>();
    if offers.len() != 1 {
        return Err(CliError::Other(
            "recipient withdrew or published an invalid offer".into(),
        ));
    }
    let offer_text = offers.into_iter().next().ok_or_else(|| {
        CliError::Other("recipient withdrew or published an invalid offer".into())
    })?;
    let offer = Offer::from_str(&offer_text).map_err(|error| {
        CliError::Other(format!("recipient published an invalid offer: {error:?}"))
    })?;
    if offer.to_string() != offer_text {
        return Err(CliError::Other(
            "recipient published a non-canonical offer".into(),
        ));
    }
    Ok((event, offer))
}

fn bip321_offer_uri(offer: Offer) -> String {
    Bip321Uri {
        offer: Some(LexeOffer(offer)),
        ..Default::default()
    }
    .to_string()
}

async fn require_nwc_pay(client: &BuzzClient, owner: &PublicKey) -> Result<(), CliError> {
    let owner = owner.to_hex();
    let raw = client
        .query(&json!({
            "kinds": [KIND_NWC_INFO],
            "authors": [owner],
            "limit": 1
        }))
        .await?;
    let events: Vec<Event> = serde_json::from_str::<Vec<serde_json::Value>>(&raw)
        .map_err(|error| CliError::Other(format!("invalid NWC info response: {error}")))?
        .into_iter()
        .filter_map(|value| Event::from_json(value.to_string()).ok())
        .filter(|event| event.verify().is_ok() && event.pubkey.to_hex() == owner)
        .collect();
    let info = events
        .into_iter()
        .max_by_key(|event| event.created_at)
        .ok_or_else(|| CliError::Other("the owner wallet has not advertised NWC support".into()))?;
    let supports_nip44 = info.tags.iter().any(|tag| {
        let parts = tag.as_slice();
        parts.first().map(String::as_str) == Some("encryption")
            && parts
                .get(1)
                .is_some_and(|value| value.split_whitespace().any(|item| item == "nip44_v2"))
    });
    let supports_321 = info.tags.iter().any(|tag| {
        let parts = tag.as_slice();
        parts.first().map(String::as_str) == Some("extensions")
            && parts
                .get(1)
                .is_some_and(|value| value.split_whitespace().any(|item| item == "321"))
    });
    if !info
        .content
        .split_whitespace()
        .any(|method| method == "pay")
        || !supports_nip44
        || !supports_321
    {
        return Err(CliError::Other(
            "the owner wallet does not advertise NWC-321 pay with NIP-44".into(),
        ));
    }
    Ok(())
}

fn build_intent(
    client: &BuzzClient,
    recipient: &str,
    amount: u64,
    comment: &str,
    offer_event: &Event,
    target: Option<(&str, u32)>,
) -> Result<Event, CliError> {
    if amount == 0 {
        return Err(CliError::Usage("--amount must be greater than zero".into()));
    }
    let amount_msats = amount
        .checked_mul(1_000)
        .ok_or_else(|| CliError::Usage("--amount is too large".into()))?;
    let zap_id = hex::encode(rand::random::<[u8; 16]>());
    let offer_json = offer_event.as_json();
    let mut tags = vec![
        tag(["p", recipient])?,
        tag(["amount", amount_msats.to_string().as_str()])?,
        tag(["offer_event", offer_json.as_str()])?,
        tag(["zap_id", zap_id.as_str()])?,
    ];
    if let Some((event_id, event_kind)) = target {
        tags.push(tag(["e", event_id])?);
        tags.push(tag(["k", event_kind.to_string().as_str()])?);
    }
    EventBuilder::new(Kind::Custom(KIND_BOLT12_ZAP_INTENT as u16), comment.trim())
        .tags(tags)
        .sign_with_keys(client.keys())
        .map_err(|error| CliError::Other(format!("failed to sign zap intent: {error}")))
}

async fn zap(
    client: &BuzzClient,
    recipient: String,
    amount: u64,
    comment: String,
    event: Option<String>,
    event_kind: Option<u32>,
    wait_seconds: u64,
) -> Result<(), CliError> {
    let recipient_key = PublicKey::from_hex(&recipient)
        .map_err(|error| CliError::Usage(format!("invalid recipient pubkey: {error}")))?;
    let recipient = recipient_key.to_hex();
    let target = match (event.as_deref(), event_kind) {
        (None, None) => None,
        (Some(id), Some(kind)) => Some((
            EventId::from_hex(id)
                .map_err(|error| CliError::Usage(format!("invalid event id: {error}")))?
                .to_hex(),
            kind,
        )),
        _ => {
            return Err(CliError::Usage(
                "--event and --event-kind must be supplied together".into(),
            ))
        }
    };
    let owner_hex = client
        .auth_tag_owner_hex()
        .ok_or_else(|| CliError::Auth("agent wallet requests require BUZZ_AUTH_TAG".into()))?;
    let owner = PublicKey::from_hex(&owner_hex)
        .map_err(|error| CliError::Auth(format!("invalid owner pubkey: {error}")))?;
    require_nwc_pay(client, &owner).await?;
    let (offer_event, offer) = latest_offer(client, &recipient).await?;
    let target_ref = target.as_ref().map(|(id, kind)| (id.as_str(), *kind));
    let intent = build_intent(
        client,
        &recipient,
        amount,
        &comment,
        &offer_event,
        target_ref,
    )?;
    let amount_msats = amount
        .checked_mul(1_000)
        .ok_or_else(|| CliError::Usage("--amount is too large".into()))?;
    let payer_note = format!("nostr:nipB1:{}", intent.id.to_hex());
    let builder = build_pay_request(
        client.keys(),
        &owner,
        NwcPayParams {
            payment: bip321_offer_uri(offer),
            amount: amount_msats,
            payer_note: Some(payer_note),
            metadata: serde_json::Map::from_iter([
                (
                    "zap_intent".into(),
                    serde_json::Value::String(intent.as_json()),
                ),
                (
                    "offer_event".into(),
                    serde_json::Value::String(offer_event.as_json()),
                ),
            ]),
        },
        now().saturating_add(wait_seconds.max(1)),
    )
    .map_err(|error| CliError::Other(error.to_string()))?;
    let request = client.sign_event(builder)?;

    let mut connection = NostrWsConnection::connect_authenticated(
        &ws_url(client.relay_url()),
        client.keys(),
        client.auth_tag(),
    )
    .await
    .map_err(|error| CliError::Transport(error.to_string()))?;
    let subscription_id = format!("nwc-{}", uuid::Uuid::new_v4().simple());
    connection
        .send_raw(&json!(["REQ", subscription_id, {
            "kinds": [KIND_NWC_RESPONSE],
            "#p": [client.keys().public_key().to_hex()],
            "since": now()
        }]))
        .await
        .map_err(|error| CliError::Transport(error.to_string()))?;
    let accepted = connection
        .send_event(request.clone())
        .await
        .map_err(|error| CliError::Transport(error.to_string()))?;
    if !accepted.accepted {
        return Err(CliError::Other(format!(
            "relay rejected NWC request: {}",
            accepted.message
        )));
    }

    let deadline = tokio::time::Instant::now() + Duration::from_secs(wait_seconds.max(1));
    loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            return Err(CliError::DeliveryUnknown(
                "timed out waiting for the wallet response".into(),
            ));
        }
        let message = connection.next_event(remaining).await.map_err(|error| {
            CliError::DeliveryUnknown(format!(
                "lost the wallet response after the relay accepted the request: {error}"
            ))
        })?;
        let RelayMessage::Event { event, .. } = message else {
            continue;
        };
        if event.pubkey != owner
            || !event.tags.iter().any(|tag| {
                let parts = tag.as_slice();
                parts.first().map(String::as_str) == Some("e")
                    && parts.get(1).map(String::as_str) == Some(request.id.to_hex().as_str())
            })
        {
            continue;
        }
        let response = decrypt_pay_response(&event, client.keys())
            .map_err(|error| CliError::Other(error.to_string()))?;
        if let Some(error) = response.error {
            return Err(CliError::Other(format!(
                "wallet {}: {}",
                error.code, error.message
            )));
        }
        let result = response
            .result
            .ok_or_else(|| CliError::Other("wallet returned no result".into()))?;
        let target_ref = target.as_ref().map(|(id, kind)| (id.as_str(), *kind));
        let channel_id = target_channel_id(client, target_ref).await?;
        let proof = build_zap_proof(client, &intent, &result, channel_id.as_deref())?;
        let accepted = connection
            .send_event(proof)
            .await
            .map_err(|error| CliError::Transport(error.to_string()))?;
        if !accepted.accepted {
            return Err(CliError::Other(format!(
                "relay rejected zap proof: {}",
                accepted.message
            )));
        }
        println!(
            "{}",
            serde_json::to_string(&json!({
                "request_event_id": request.id.to_hex(),
                "intent_event_id": intent.id.to_hex(),
                "payment": result,
                "proof_published": true
            }))
            .map_err(|error| CliError::Other(error.to_string()))?
        );
        return Ok(());
    }
}

pub async fn dispatch(command: WalletCmd, client: &BuzzClient) -> Result<(), CliError> {
    match command {
        WalletCmd::Zap {
            recipient,
            amount,
            comment,
            event,
            event_kind,
            wait_seconds,
        } => {
            zap(
                client,
                recipient,
                amount,
                comment,
                event,
                event_kind,
                wait_seconds,
            )
            .await
        }
    }
}

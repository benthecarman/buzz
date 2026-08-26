//! Agent-facing NWC wallet requests.

use std::{
    io::Write,
    path::{Path, PathBuf},
    str::FromStr,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use buzz_core::{
    kind::{
        KIND_BOLT12_OFFER, KIND_BOLT12_ZAP, KIND_BOLT12_ZAP_INTENT, KIND_NWC_INFO,
        KIND_NWC_RESPONSE,
    },
    nwc::{
        build_get_balance_request, build_pay_request, decrypt_get_balance_response,
        decrypt_pay_response, NwcPayParams, NwcPayResult,
    },
};
use buzz_ws_client::{NostrWsConnection, RelayMessage};
use lexe_api_core::types::offer::Offer as LexeOffer;
use lexe_payment_uri_core::Bip321Uri;
use lightning::offers::offer::Offer;
use lightning_payer_proof::verify;
use nostr::{Event, EventBuilder, EventId, JsonUtil, Kind, PublicKey, Tag};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::{client::BuzzClient, error::CliError, WalletAmount, WalletCmd};

const APPROVAL_WINDOW_SECONDS: u64 = 600;
const RECOVERY_VERSION: u8 = 1;

#[derive(Clone, Debug, Deserialize, Serialize)]
struct WalletRecoveryRecord {
    version: u8,
    relay_url: String,
    owner_pubkey: String,
    request_event_json: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    intent_event_json: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    proof_event_json: Option<String>,
}

fn recovery_root() -> Result<PathBuf, CliError> {
    dirs::data_dir()
        .map(|path| path.join("buzz").join("wallet-requests"))
        .ok_or_else(|| CliError::Other("cannot resolve the wallet recovery directory".into()))
}

fn recovery_path_at(
    root: &Path,
    client_pubkey: &str,
    request_id: &str,
) -> Result<PathBuf, CliError> {
    let request_id = EventId::from_hex(request_id)
        .map_err(|error| CliError::Usage(format!("invalid wallet request id: {error}")))?
        .to_hex();
    Ok(root.join(client_pubkey).join(format!("{request_id}.json")))
}

fn recovery_path(client: &BuzzClient, request_id: &str) -> Result<PathBuf, CliError> {
    recovery_path_at(
        &recovery_root()?,
        &client.keys().public_key().to_hex(),
        request_id,
    )
}

fn store_recovery_at(path: &Path, record: &WalletRecoveryRecord) -> Result<(), CliError> {
    let parent = path
        .parent()
        .ok_or_else(|| CliError::Other("wallet recovery path has no parent".into()))?;
    std::fs::create_dir_all(parent)
        .map_err(|error| CliError::Other(format!("create wallet recovery directory: {error}")))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o700)).map_err(
            |error| CliError::Other(format!("restrict wallet recovery directory: {error}")),
        )?;
    }
    let bytes = serde_json::to_vec_pretty(record)
        .map_err(|error| CliError::Other(format!("encode wallet recovery record: {error}")))?;
    let file = atomic_write_file::AtomicWriteFile::open(path)
        .map_err(|error| CliError::Other(format!("open wallet recovery record: {error}")))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        file.set_permissions(std::fs::Permissions::from_mode(0o600))
            .map_err(|error| {
                CliError::Other(format!("restrict wallet recovery record: {error}"))
            })?;
    }
    let mut file = file;
    file.write_all(&bytes)
        .map_err(|error| CliError::Other(format!("write wallet recovery record: {error}")))?;
    file.commit()
        .map_err(|error| CliError::Other(format!("commit wallet recovery record: {error}")))
}

fn store_recovery(client: &BuzzClient, record: &WalletRecoveryRecord) -> Result<(), CliError> {
    let request = Event::from_json(&record.request_event_json)
        .map_err(|error| CliError::Other(format!("invalid wallet recovery request: {error}")))?;
    store_recovery_at(&recovery_path(client, &request.id.to_hex())?, record)
}

fn load_recovery(client: &BuzzClient, request_id: &str) -> Result<WalletRecoveryRecord, CliError> {
    let path = recovery_path(client, request_id)?;
    let bytes = std::fs::read(&path).map_err(|error| {
        if error.kind() == std::io::ErrorKind::NotFound {
            CliError::NotFound(format!("no wallet recovery record for {request_id}"))
        } else {
            CliError::Other(format!("read wallet recovery record: {error}"))
        }
    })?;
    let record: WalletRecoveryRecord = serde_json::from_slice(&bytes)
        .map_err(|error| CliError::Other(format!("decode wallet recovery record: {error}")))?;
    if record.version != RECOVERY_VERSION {
        return Err(CliError::Other(format!(
            "unsupported wallet recovery version {}",
            record.version
        )));
    }
    Ok(record)
}

fn remove_recovery(client: &BuzzClient, request_id: &str) -> Result<(), CliError> {
    match std::fs::remove_file(recovery_path(client, request_id)?) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(CliError::Other(format!(
            "remove wallet recovery record: {error}"
        ))),
    }
}

fn recovery_error(error: CliError, request_id: &str) -> CliError {
    match error {
        CliError::Transport(message) | CliError::DeliveryUnknown(message) => {
            CliError::DeliveryUnknown(format!(
                "{message}; request {request_id} can be resumed with `buzz wallet status {request_id}`"
            ))
        }
        other => other,
    }
}

fn proof_recovery_error(error: CliError, request_id: &str) -> CliError {
    CliError::DeliveryUnknown(format!(
        "payment settled but zap proof publication is unresolved: {error}; request {request_id} \
         can be resumed with `buzz wallet status {request_id}`"
    ))
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or_default()
}

fn first_canonical_offer(event: &Event) -> Option<Offer> {
    event.tags.iter().find_map(|tag| {
        let parts = tag.as_slice();
        if parts.first().map(String::as_str) != Some("offer") {
            return None;
        }
        let value = parts.get(1)?;
        let offer = Offer::from_str(value).ok()?;
        (offer.to_string() == *value).then_some(offer)
    })
}

fn tag(parts: impl IntoIterator<Item = impl Into<String>>) -> Result<Tag, CliError> {
    Tag::parse(parts).map_err(|error| CliError::Other(format!("invalid zap tag: {error}")))
}

fn zap_target_filter(event_id: &str) -> serde_json::Value {
    json!({
        "ids": [event_id],
        "limit": 1
    })
}

fn build_zap_proof(
    client: &BuzzClient,
    intent: &Event,
    result: &NwcPayResult,
) -> Result<Event, CliError> {
    if result.state != "settled" {
        return Err(CliError::Other(format!(
            "wallet returned non-settled zap state: {}",
            result.state
        )));
    }
    let proof = result
        .payer_proof
        .as_deref()
        .ok_or_else(|| CliError::Other("wallet returned no BOLT12 payer proof".to_string()))?;
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
    verify(proof).map_err(|error| {
        CliError::Other(format!("wallet returned an invalid payer proof: {error}"))
    })?;
    EventBuilder::new(Kind::Custom(KIND_BOLT12_ZAP as u16), &intent.content)
        .tags(tags)
        .sign_with_keys(client.keys())
        .map_err(|error| CliError::Other(format!("failed to sign zap proof: {error}")))
}

async fn publish_zap_proof(client: &BuzzClient, proof: Event) -> Result<(), CliError> {
    let mut connection = NostrWsConnection::connect_authenticated(
        &ws_url(client.relay_url()),
        client.keys(),
        client.auth_tag(),
    )
    .await
    .map_err(|error| CliError::Transport(error.to_string()))?;
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
    Ok(())
}

async fn zap_target_event(client: &BuzzClient, event_id: &str) -> Result<Event, CliError> {
    let event_id = EventId::from_hex(event_id)
        .map_err(|error| CliError::Usage(format!("invalid event id: {error}")))?
        .to_hex();
    let raw = client.query(&zap_target_filter(&event_id)).await?;
    let values: Vec<serde_json::Value> = serde_json::from_str(&raw)
        .map_err(|error| CliError::Other(format!("invalid zap target response: {error}")))?;
    let event = values
        .into_iter()
        .find_map(|value| Event::from_json(value.to_string()).ok())
        .ok_or_else(|| CliError::Other("zap target event was not found".into()))?;
    event
        .verify()
        .map_err(|error| CliError::Other(format!("invalid zap target event: {error}")))?;
    if event.id.to_hex() != event_id {
        return Err(CliError::Other(
            "relay returned the wrong zap target".into(),
        ));
    }
    Ok(event)
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
    let offer = first_canonical_offer(&event).ok_or_else(|| {
        CliError::Other("recipient withdrew or published no canonical offer".into())
    })?;
    Ok((event, offer))
}

fn bip321_offer_uri(offer: Offer) -> String {
    Bip321Uri {
        offer: Some(LexeOffer(offer)),
        ..Default::default()
    }
    .to_string()
}

fn payment_request(
    payment: &str,
    amount: Option<WalletAmount>,
) -> Result<(String, Option<u64>), CliError> {
    let payment = payment.trim();
    if payment.is_empty() {
        return Err(CliError::Usage("payment must not be empty".into()));
    }
    let amount_msats = amount.map(WalletAmount::millisatoshis);
    Ok((payment.to_string(), amount_msats))
}

fn payment_result_json(result: &NwcPayResult) -> Result<serde_json::Value, CliError> {
    let mut value = serde_json::to_value(result)
        .map_err(|error| CliError::Other(format!("invalid wallet result: {error}")))?;
    let fields = value
        .as_object_mut()
        .ok_or_else(|| CliError::Other("wallet result must be an object".into()))?;
    fields.insert(
        "amount".into(),
        serde_json::Value::String(format!("{}msats", result.amount)),
    );
    if let Some(fees_paid) = result.fees_paid {
        fields.insert(
            "fees_paid".into(),
            serde_json::Value::String(format!("{fees_paid}msats")),
        );
    }
    Ok(value)
}

fn zap_amount_msats(amount: WalletAmount) -> Result<u64, CliError> {
    let amount_msats = amount.millisatoshis();
    if !amount_msats.is_multiple_of(1_000) {
        return Err(CliError::Usage(
            "zap amount must be a whole number of satoshis".into(),
        ));
    }
    Ok(amount_msats)
}

async fn require_nwc_method(
    client: &BuzzClient,
    owner: &PublicKey,
    method: &str,
) -> Result<(), CliError> {
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
    let supports_method = info
        .content
        .split_whitespace()
        .any(|advertised| advertised == method);
    if !supports_method || !supports_nip44 || (method == "pay" && !supports_321) {
        return Err(CliError::Other(format!(
            "the owner wallet does not advertise NWC {method} with NIP-44"
        )));
    }
    Ok(())
}

fn build_intent(
    client: &BuzzClient,
    recipient: &str,
    amount_msats: u64,
    comment: &str,
    offer_event: &Event,
    target: Option<&Event>,
) -> Result<Event, CliError> {
    let zap_id = hex::encode(rand::random::<[u8; 16]>());
    let offer_json = offer_event.as_json();
    let mut tags = vec![
        tag(["p", recipient])?,
        tag(["amount", amount_msats.to_string().as_str()])?,
        tag(["offer_event", offer_json.as_str()])?,
        tag(["zap_id", zap_id.as_str()])?,
    ];
    if let Some(event) = target {
        let event_id = event.id.to_hex();
        tags.push(tag(["e", event_id.as_str()])?);
        if let Some(channel_id) = event.tags.iter().find_map(|tag| {
            let parts = tag.as_slice();
            (parts.first().map(String::as_str) == Some("h"))
                .then(|| parts.get(1).map(String::as_str))
                .flatten()
        }) {
            tags.push(tag(["h", channel_id])?);
        }
        let event_kind = u32::from(event.kind.as_u16()).to_string();
        tags.push(tag(["k", event_kind.as_str()])?);
    }
    EventBuilder::new(Kind::Custom(KIND_BOLT12_ZAP_INTENT as u16), comment.trim())
        .tags(tags)
        .sign_with_keys(client.keys())
        .map_err(|error| CliError::Other(format!("failed to sign zap intent: {error}")))
}

async fn zap(
    client: &BuzzClient,
    recipient: Option<String>,
    amount: WalletAmount,
    comment: String,
    event: Option<String>,
    wait_seconds: u64,
) -> Result<(), CliError> {
    let amount_msats = zap_amount_msats(amount)?;
    let target = match event {
        Some(event_id) => Some(zap_target_event(client, &event_id).await?),
        None => None,
    };
    let recipient = match (recipient, target.as_ref()) {
        (Some(recipient), None) => PublicKey::parse(&recipient)
            .map_err(|error| CliError::Usage(format!("invalid recipient pubkey: {error}")))?
            .to_hex(),
        (None, Some(event)) => event.pubkey.to_hex(),
        _ => return Err(CliError::Usage("select one zap target".into())),
    };
    let owner = wallet_owner(client, "pay").await?;
    let (offer_event, offer) = latest_offer(client, &recipient).await?;
    let intent = build_intent(
        client,
        &recipient,
        amount_msats,
        &comment,
        &offer_event,
        target.as_ref(),
    )?;
    let payer_note = format!("nostr:nipB1:{}", intent.id.to_hex());
    let (request, result) = request_payment(
        client,
        &owner,
        NwcPayParams {
            payment: bip321_offer_uri(offer),
            amount: Some(amount_msats),
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
        wait_seconds,
        Some(&intent),
    )
    .await?;

    if result.state != "settled" {
        let request_id = request.id.to_hex();
        if result.state == "failed" {
            remove_recovery(client, &request_id)?;
        }
        let payment = payment_result_json(&result)?;
        println!(
            "{}",
            serde_json::to_string(&json!({
                "request_event_id": request_id,
                "intent_event_id": intent.id.to_hex(),
                "payment": payment,
                "proof_published": false
            }))
            .map_err(|error| CliError::Other(error.to_string()))?
        );
        return Ok(());
    }

    let request_id = request.id.to_hex();
    let proof = build_zap_proof(client, &intent, &result)
        .map_err(|error| proof_recovery_error(error, &request_id))?;
    let mut recovery = load_recovery(client, &request_id)?;
    recovery.proof_event_json = Some(proof.as_json());
    store_recovery(client, &recovery)?;
    publish_zap_proof(client, proof)
        .await
        .map_err(|error| proof_recovery_error(error, &request_id))?;
    remove_recovery(client, &request_id)?;
    let payment = payment_result_json(&result)?;
    println!(
        "{}",
        serde_json::to_string(&json!({
            "request_event_id": request.id.to_hex(),
            "intent_event_id": intent.id.to_hex(),
            "payment": payment,
            "proof_published": true
        }))
        .map_err(|error| CliError::Other(error.to_string()))?
    );
    Ok(())
}

async fn wallet_owner(client: &BuzzClient, method: &str) -> Result<PublicKey, CliError> {
    let owner_hex = client
        .auth_tag_owner_hex()
        .ok_or_else(|| CliError::Auth("agent wallet requests require BUZZ_AUTH_TAG".into()))?;
    let owner = PublicKey::from_hex(&owner_hex)
        .map_err(|error| CliError::Auth(format!("invalid owner pubkey: {error}")))?;
    require_nwc_method(client, &owner, method).await?;
    Ok(owner)
}

async fn send_nwc_request(
    client: &BuzzClient,
    owner: &PublicKey,
    builder: EventBuilder,
    wait_seconds: u64,
) -> Result<(Event, Event), CliError> {
    let request = client.sign_event(builder)?;
    let response = send_signed_nwc_request(client, owner, &request, wait_seconds).await?;
    Ok((request, response))
}

async fn send_signed_nwc_request(
    client: &BuzzClient,
    owner: &PublicKey,
    request: &Event,
    wait_seconds: u64,
) -> Result<Event, CliError> {
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
        if event.pubkey != *owner
            || !event.tags.iter().any(|tag| {
                let parts = tag.as_slice();
                parts.first().map(String::as_str) == Some("e")
                    && parts.get(1).map(String::as_str) == Some(request.id.to_hex().as_str())
            })
        {
            continue;
        }
        return Ok(*event);
    }
}

async fn request_payment(
    client: &BuzzClient,
    owner: &PublicKey,
    params: NwcPayParams,
    wait_seconds: u64,
    intent: Option<&Event>,
) -> Result<(Event, NwcPayResult), CliError> {
    let expires_at = now().saturating_add(APPROVAL_WINDOW_SECONDS);
    let builder = build_pay_request(client.keys(), owner, params, expires_at)
        .map_err(|error| CliError::Other(error.to_string()))?;
    let request = client.sign_event(builder)?;
    store_recovery(
        client,
        &WalletRecoveryRecord {
            version: RECOVERY_VERSION,
            relay_url: client.relay_url().trim_end_matches('/').to_string(),
            owner_pubkey: owner.to_hex(),
            request_event_json: request.as_json(),
            intent_event_json: intent.map(JsonUtil::as_json),
            proof_event_json: None,
        },
    )?;
    let event = match send_signed_nwc_request(client, owner, &request, wait_seconds).await {
        Ok(event) => event,
        Err(error @ (CliError::Transport(_) | CliError::DeliveryUnknown(_))) => {
            return Err(recovery_error(error, &request.id.to_hex()));
        }
        Err(error) => {
            remove_recovery(client, &request.id.to_hex())?;
            return Err(error);
        }
    };
    let response = decrypt_pay_response(&event, client.keys())
        .map_err(|error| CliError::Other(error.to_string()))?;
    if let Some(error) = response.error {
        if error.code != "PAYMENT_STATUS_UNKNOWN" {
            remove_recovery(client, &request.id.to_hex())?;
        }
        return Err(CliError::Other(format!(
            "wallet {}: {}",
            error.code, error.message
        )));
    }
    response
        .result
        .map(|result| (request, result))
        .ok_or_else(|| CliError::Other("wallet returned no result".into()))
}

async fn pay(
    client: &BuzzClient,
    payment: String,
    amount: Option<WalletAmount>,
    wait_seconds: u64,
) -> Result<(), CliError> {
    let (payment, amount_msats) = payment_request(&payment, amount)?;
    let owner = wallet_owner(client, "pay").await?;
    let (request, result) = request_payment(
        client,
        &owner,
        NwcPayParams {
            payment,
            amount: amount_msats,
            payer_note: None,
            metadata: Default::default(),
        },
        wait_seconds,
        None,
    )
    .await?;
    if result.state != "pending" {
        remove_recovery(client, &request.id.to_hex())?;
    }
    let payment = payment_result_json(&result)?;
    println!(
        "{}",
        serde_json::to_string(&json!({
            "request_event_id": request.id.to_hex(),
            "payment": payment
        }))
        .map_err(|error| CliError::Other(error.to_string()))?
    );
    Ok(())
}

async fn status(
    client: &BuzzClient,
    request_event_id: String,
    wait_seconds: u64,
) -> Result<(), CliError> {
    let request_event_id = EventId::from_hex(&request_event_id)
        .map_err(|error| CliError::Usage(format!("invalid wallet request id: {error}")))?
        .to_hex();
    let mut recovery = load_recovery(client, &request_event_id)?;
    let relay_url = client.relay_url().trim_end_matches('/');
    if recovery.relay_url != relay_url {
        return Err(CliError::Usage(format!(
            "wallet request belongs to {}, not {relay_url}",
            recovery.relay_url
        )));
    }
    let owner = wallet_owner(client, "pay").await?;
    if recovery.owner_pubkey != owner.to_hex() {
        return Err(CliError::Auth(
            "wallet request belongs to a different owner wallet".into(),
        ));
    }
    let request = Event::from_json(&recovery.request_event_json)
        .map_err(|error| CliError::Other(format!("invalid wallet recovery request: {error}")))?;
    request
        .verify()
        .map_err(|error| CliError::Other(format!("invalid wallet recovery signature: {error}")))?;
    if request.id.to_hex() != request_event_id
        || request.pubkey != client.keys().public_key()
        || request.kind != Kind::Custom(buzz_core::kind::KIND_NWC_REQUEST as u16)
    {
        return Err(CliError::Other(
            "wallet recovery record does not match this request and identity".into(),
        ));
    }
    let event = send_signed_nwc_request(client, &owner, &request, wait_seconds)
        .await
        .map_err(|error| recovery_error(error, &request_event_id))?;
    let response = decrypt_pay_response(&event, client.keys())
        .map_err(|error| CliError::Other(error.to_string()))?;
    if let Some(error) = response.error {
        if error.code != "PAYMENT_STATUS_UNKNOWN" {
            remove_recovery(client, &request_event_id)?;
        }
        return Err(CliError::Other(format!(
            "wallet {}: {}",
            error.code, error.message
        )));
    }
    let result = response
        .result
        .ok_or_else(|| CliError::Other("wallet returned no result".into()))?;
    let mut proof_published = false;
    if result.state == "settled" {
        if let Some(intent_json) = recovery.intent_event_json.as_deref() {
            let intent = Event::from_json(intent_json).map_err(|error| {
                CliError::Other(format!("invalid recovered zap intent: {error}"))
            })?;
            intent.verify().map_err(|error| {
                CliError::Other(format!("invalid recovered zap intent signature: {error}"))
            })?;
            if intent.pubkey != client.keys().public_key() {
                return Err(CliError::Other(
                    "recovered zap intent belongs to a different identity".into(),
                ));
            }
            let proof = match recovery.proof_event_json.as_deref() {
                Some(proof_json) => Event::from_json(proof_json).map_err(|error| {
                    CliError::Other(format!("invalid recovered zap proof: {error}"))
                })?,
                None => {
                    let proof = build_zap_proof(client, &intent, &result)
                        .map_err(|error| proof_recovery_error(error, &request_event_id))?;
                    recovery.proof_event_json = Some(proof.as_json());
                    store_recovery(client, &recovery)?;
                    proof
                }
            };
            proof.verify().map_err(|error| {
                CliError::Other(format!("invalid recovered zap proof signature: {error}"))
            })?;
            if proof.pubkey != client.keys().public_key()
                || proof.kind != Kind::Custom(KIND_BOLT12_ZAP as u16)
            {
                return Err(CliError::Other(
                    "recovered zap proof does not match this identity and event kind".into(),
                ));
            }
            publish_zap_proof(client, proof)
                .await
                .map_err(|error| proof_recovery_error(error, &request_event_id))?;
            proof_published = true;
        }
        remove_recovery(client, &request_event_id)?;
    } else if result.state == "failed" {
        remove_recovery(client, &request_event_id)?;
    }
    let payment = payment_result_json(&result)?;
    println!(
        "{}",
        serde_json::to_string(&json!({
            "request_event_id": request.id.to_hex(),
            "payment": payment,
            "proof_published": proof_published,
            "recovered": true
        }))
        .map_err(|error| CliError::Other(error.to_string()))?
    );
    Ok(())
}

async fn balance(client: &BuzzClient, wait_seconds: u64) -> Result<(), CliError> {
    let owner = wallet_owner(client, "get_balance").await?;
    let builder = build_get_balance_request(
        client.keys(),
        &owner,
        now().saturating_add(APPROVAL_WINDOW_SECONDS),
    )
    .map_err(|error| CliError::Other(error.to_string()))?;
    let (request, event) = send_nwc_request(client, &owner, builder, wait_seconds).await?;
    let response = decrypt_get_balance_response(&event, client.keys())
        .map_err(|error| CliError::Other(error.to_string()))?;
    if let Some(error) = response.error {
        return Err(CliError::Other(format!(
            "wallet {}: {}",
            error.code, error.message
        )));
    }
    let result = response
        .result
        .ok_or_else(|| CliError::Other("wallet returned no balance".into()))?;
    println!(
        "{}",
        serde_json::to_string(&json!({
            "request_event_id": request.id.to_hex(),
            "balance": format!("{}msats", result.balance)
        }))
        .map_err(|error| CliError::Other(error.to_string()))?
    );
    Ok(())
}

pub async fn dispatch(command: WalletCmd, client: &BuzzClient) -> Result<(), CliError> {
    match command {
        WalletCmd::Balance { wait_seconds } => balance(client, wait_seconds).await,
        WalletCmd::Pay {
            payment,
            amount,
            wait_seconds,
        } => pay(client, payment, amount, wait_seconds).await,
        WalletCmd::Status {
            request_event_id,
            wait_seconds,
        } => status(client, request_event_id, wait_seconds).await,
        WalletCmd::Zap {
            recipient,
            amount,
            comment,
            event,
            wait_seconds,
        } => zap(client, recipient, amount, comment, event, wait_seconds).await,
    }
}

#[cfg(test)]
mod tests {
    use buzz_core::nwc::NwcPayResult;
    use nostr::{EventBuilder, Keys, Kind, Tag};

    use super::{
        first_canonical_offer, payment_request, payment_result_json, recovery_path_at,
        store_recovery_at, zap_amount_msats, zap_target_filter, WalletAmount, WalletRecoveryRecord,
        KIND_BOLT12_OFFER, RECOVERY_VERSION,
    };

    const VALID_OFFER: &str = include_str!("../../../buzz-core/test-vectors/payer-proof-offer.txt");
    const VALID_INVOICE: &str = "lnbc1gcssw9pdqqpp54dkfmzgm5cqz4hzz24mpl7xtgz55dsuh430ap4rlugvywlm4syhqsp5qqtk8n0x2wa6ajl32mp6hj8u9vs55s5lst4s2rws3he4622w08es9qyysgqcqypt3ffpp36sw424yacusmj3hy32df9g97nlwm0a3e0yxw4nd8uau2zdw85lfl5w0h3mggd5g3qswxr9lje0el8g98vul9yec59gf0zxu3eg9rhda09ducxpupsfh36ks9jez7aamsn7hpkxqpw2xyek";

    #[test]
    fn offer_selection_skips_invalid_tags() {
        let valid_offer = VALID_OFFER.trim();
        let event = EventBuilder::new(Kind::Custom(KIND_BOLT12_OFFER as u16), "")
            .tags([
                Tag::parse(["offer", "invalid-offer"]).expect("invalid offer tag"),
                Tag::parse(["offer", valid_offer]).expect("valid offer tag"),
            ])
            .sign_with_keys(&Keys::generate())
            .expect("sign offer announcement");

        assert_eq!(
            first_canonical_offer(&event)
                .expect("find canonical offer")
                .to_string(),
            valid_offer
        );
    }

    #[test]
    fn raw_bolt11_is_forwarded_to_the_wallet() {
        let (payment, amount) =
            payment_request(VALID_INVOICE, Some("100sats".parse().unwrap())).unwrap();
        assert_eq!(payment, VALID_INVOICE);
        assert_eq!(amount, Some(100_000));
    }

    #[test]
    fn bip321_payment_is_forwarded_without_losing_instructions() {
        let payment = format!("bitcoin:?lno={VALID_OFFER}&future=opaque");
        let (forwarded, amount) =
            payment_request(&payment, Some("100000msats".parse().unwrap())).unwrap();

        assert_eq!(forwarded, payment);
        assert_eq!(amount, Some(100_000));
    }

    #[test]
    fn payment_amount_can_be_selected_by_the_wallet() {
        let (payment, amount) = payment_request(VALID_INVOICE, None).unwrap();

        assert_eq!(payment, VALID_INVOICE);
        assert_eq!(amount, None);
    }

    #[test]
    fn wallet_amount_requires_an_explicit_unit() {
        assert_eq!(
            "50sats".parse::<WalletAmount>().unwrap().millisatoshis(),
            50_000
        );
        assert_eq!(
            "50000msats"
                .parse::<WalletAmount>()
                .unwrap()
                .millisatoshis(),
            50_000
        );
        assert!("50".parse::<WalletAmount>().is_err());
        assert!("0sats".parse::<WalletAmount>().is_err());
        assert!("1.5sats".parse::<WalletAmount>().is_err());
        assert!("-1sats".parse::<WalletAmount>().is_err());
        assert!("18446744073709551615sats".parse::<WalletAmount>().is_err());
    }

    #[test]
    fn zap_amount_requires_whole_satoshis() {
        assert_eq!(
            zap_amount_msats("50000msats".parse().unwrap()).unwrap(),
            50_000
        );
        assert!(zap_amount_msats("50001msats".parse().unwrap()).is_err());
    }

    #[test]
    fn zap_target_lookup_uses_the_event_id_exemption() {
        let event_id = "ab".repeat(32);
        let filter = zap_target_filter(&event_id);

        assert_eq!(filter["ids"], serde_json::json!([event_id]));
        assert_eq!(filter["limit"], 1);
        assert!(filter.get("kinds").is_none());
    }

    #[test]
    fn payment_result_qualifies_millisatoshi_values() {
        let result = NwcPayResult {
            transaction_id: "payment-1".into(),
            state: "settled".into(),
            instruction_type: "bolt11".into(),
            amount: 50_000,
            fees_paid: Some(1_000),
            preimage: Some("00".repeat(32)),
            payer_proof: None,
            txid: None,
            failure_reason: None,
            created_at: 1,
            settled_at: Some(2),
        };

        let value = payment_result_json(&result).unwrap();

        assert_eq!(value["amount"], "50000msats");
        assert_eq!(value["fees_paid"], "1000msats");
        assert_eq!(value["preimage"], "00".repeat(32));
        assert!(value.get("payer_proof").is_none());
    }

    #[test]
    fn recovery_records_are_atomic_private_and_identity_scoped() {
        let root = tempfile::tempdir().unwrap();
        let request_id = "ab".repeat(32);
        let path = recovery_path_at(root.path(), "client", &request_id).unwrap();
        let record = WalletRecoveryRecord {
            version: RECOVERY_VERSION,
            relay_url: "https://relay.example".into(),
            owner_pubkey: "owner".into(),
            request_event_json: "request".into(),
            intent_event_json: Some("intent".into()),
            proof_event_json: None,
        };

        store_recovery_at(&path, &record).unwrap();

        let decoded: WalletRecoveryRecord =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(decoded.request_event_json, "request");
        assert_eq!(
            path,
            root.path()
                .join("client")
                .join(format!("{request_id}.json"))
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(path).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
    }
}

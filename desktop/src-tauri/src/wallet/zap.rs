use std::{
    io::Write,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use atomic_write_file::AtomicWriteFile;
use buzz_conformance_pkg::wallet::{WalletAbstractState, WalletAttemptStatus, WalletTraceAction};
use buzz_core_pkg::kind::{KIND_BOLT12_OFFER, KIND_BOLT12_ZAP, KIND_BOLT12_ZAP_INTENT};
use nostr::{Event, EventBuilder, JsonUtil, Keys, Kind, Tag};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::models::{
    WalletError, WalletPaymentResult, WalletPlaceholderMessageZap, WalletProfileZapDraft,
    WalletProfileZapResult, WalletRecipientOffer, WalletVerifiedZapEvent,
};
use super::{conformance, lexe_provider::canonical_offer};

const TERMINAL_ATTEMPT_RETENTION_MS: u64 = 90 * 24 * 60 * 60 * 1_000;
/// Temporary proof marker published until the wallet exposes a real `lnp` proof.
pub(crate) const PLACEHOLDER_PAYER_PROOF: &str = "placeholder";

fn tag(parts: impl IntoIterator<Item = impl Into<String>>) -> Result<Tag, WalletError> {
    Tag::parse(parts).map_err(|error| WalletError::new("invalid_zap", error.to_string()))
}

fn amount_msats(amount: u64) -> Result<String, WalletError> {
    if amount == 0 {
        return Err(WalletError::new(
            "invalid_amount",
            "Bitcoin amount must be greater than zero",
        ));
    }
    amount
        .checked_mul(1_000)
        .map(|amount| amount.to_string())
        .ok_or_else(|| WalletError::new("invalid_amount", "Bitcoin amount is too large"))
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis().min(u128::from(u64::MAX)) as u64)
        .unwrap_or_default()
}

pub fn build_offer_announcement(offer: &str) -> Result<EventBuilder, WalletError> {
    validate_canonical_offer(offer)?;
    Ok(EventBuilder::new(Kind::Custom(KIND_BOLT12_OFFER as u16), "").tag(tag(["offer", offer])?))
}

pub fn build_offer_withdrawal() -> EventBuilder {
    EventBuilder::new(Kind::Custom(KIND_BOLT12_OFFER as u16), "")
}

/// Verify an authored offer announcement, including authoritative withdrawals.
pub(crate) fn validate_offer_event(
    event: &Event,
    recipient_pubkey: &str,
) -> Result<(), WalletError> {
    event
        .verify()
        .map_err(|error| WalletError::new("offer_invalid", error.to_string()))?;
    if event.kind != Kind::Custom(KIND_BOLT12_OFFER as u16)
        || event.pubkey.to_hex() != recipient_pubkey
    {
        return Err(WalletError::new(
            "offer_invalid",
            "offer announcement does not match the recipient",
        ));
    }
    Ok(())
}

pub fn recipient_offer(
    event: &Event,
    recipient_pubkey: &str,
) -> Result<WalletRecipientOffer, WalletError> {
    validate_offer_event(event, recipient_pubkey)?;
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
    if offers.is_empty()
        || offers
            .iter()
            .any(|offer| validate_canonical_offer(offer).is_err())
    {
        return Err(WalletError::new(
            "offer_invalid",
            "offer announcement must contain only canonical BOLT12 offers",
        ));
    }
    // The current provider accepts one offer. The signed announcement remains
    // embedded in full so a verifier can validate the proof against any offer.
    let offer = offers.into_iter().next().ok_or_else(|| {
        WalletError::new(
            "offer_invalid",
            "offer announcement has no canonical BOLT12 offer",
        )
    })?;
    Ok(WalletRecipientOffer {
        recipient_pubkey: recipient_pubkey.to_string(),
        offer,
        offer_event_json: event.as_json(),
        offer_event_id: event.id.to_hex(),
    })
}

fn exact_event_tag<'a>(event: &'a Event, name: &str) -> Result<&'a str, WalletError> {
    let matching = event
        .tags
        .iter()
        .filter(|tag| tag.as_slice().first().map(String::as_str) == Some(name))
        .collect::<Vec<_>>();
    if matching.len() != 1 {
        return Err(WalletError::new(
            "invalid_zap",
            format!("zap must contain exactly one {name} tag"),
        ));
    }
    matching[0]
        .as_slice()
        .get(1)
        .map(String::as_str)
        .ok_or_else(|| WalletError::new("invalid_zap", format!("zap {name} tag has no value")))
}

fn optional_event_tag<'a>(event: &'a Event, name: &str) -> Result<Option<&'a str>, WalletError> {
    let matching = event
        .tags
        .iter()
        .filter(|tag| tag.as_slice().first().map(String::as_str) == Some(name))
        .collect::<Vec<_>>();
    if matching.len() > 1 {
        return Err(WalletError::new(
            "invalid_zap",
            format!("zap must not contain duplicate {name} tags"),
        ));
    }
    matching
        .first()
        .map(|tag| {
            tag.as_slice().get(1).map(String::as_str).ok_or_else(|| {
                WalletError::new("invalid_zap", format!("zap {name} tag has no value"))
            })
        })
        .transpose()
}

fn matching_optional_tag(outer: &Event, intent: &Event, name: &str) -> Result<(), WalletError> {
    if optional_event_tag(outer, name)? != optional_event_tag(intent, name)? {
        return Err(WalletError::new(
            "invalid_zap",
            format!("zap {name} tag does not match its intent"),
        ));
    }
    Ok(())
}

fn valid_payer_proof_envelope(value: &str) -> bool {
    const BOLT12_ALPHABET: &str = "qpzry9x8gf2tvdw0s3jn54khce6mua7l";
    value
        .strip_prefix("lnp1")
        .is_some_and(|data| !data.is_empty() && data.chars().all(|ch| BOLT12_ALPHABET.contains(ch)))
}

/// Validate a NIP-B1 proof chain and parse every embedded BOLT12 offer through
/// rust-lightning (via Lexe's `Offer` newtype adapter).
pub(crate) fn parse_tagged_zap_event(
    raw_event: &serde_json::Value,
) -> Result<WalletVerifiedZapEvent, WalletError> {
    let event = Event::from_json(raw_event.to_string())
        .map_err(|error| WalletError::new("invalid_zap", error.to_string()))?;
    event
        .verify()
        .map_err(|error| WalletError::new("invalid_zap", error.to_string()))?;
    if event.kind != Kind::Custom(KIND_BOLT12_ZAP as u16) {
        return Err(WalletError::new("invalid_zap", "event is not a BOLT12 zap"));
    }

    let recipient = exact_event_tag(&event, "p")?.to_ascii_lowercase();
    let amount_text = exact_event_tag(&event, "amount")?;
    let description = exact_event_tag(&event, "description")?;
    let offer_event_json = exact_event_tag(&event, "offer_event")?;
    let proof = exact_event_tag(&event, "proof")?;
    if proof != PLACEHOLDER_PAYER_PROOF && !valid_payer_proof_envelope(proof) {
        return Err(WalletError::new(
            "invalid_zap",
            "zap has an invalid payer-proof envelope",
        ));
    }

    let amount_msats = amount_text
        .parse::<u64>()
        .map_err(|_| WalletError::new("invalid_zap", "zap amount is not an integer"))?;
    if amount_msats == 0 || !amount_msats.is_multiple_of(1_000) {
        return Err(WalletError::new(
            "invalid_zap",
            "zap amount must be a positive whole-satoshi value",
        ));
    }

    let intent = Event::from_json(description)
        .map_err(|error| WalletError::new("invalid_zap", error.to_string()))?;
    intent
        .verify()
        .map_err(|error| WalletError::new("invalid_zap", error.to_string()))?;
    let offer_event = Event::from_json(offer_event_json)
        .map_err(|error| WalletError::new("invalid_zap", error.to_string()))?;
    offer_event
        .verify()
        .map_err(|error| WalletError::new("invalid_zap", error.to_string()))?;

    if intent.kind != Kind::Custom(KIND_BOLT12_ZAP_INTENT as u16)
        || intent.pubkey != event.pubkey
        || intent.content != event.content
        || exact_event_tag(&intent, "p")?.to_ascii_lowercase() != recipient
        || exact_event_tag(&intent, "amount")? != amount_text
        || exact_event_tag(&intent, "offer_event")? != offer_event_json
    {
        return Err(WalletError::new(
            "invalid_zap",
            "zap does not match its signed intent",
        ));
    }

    let zap_id = exact_event_tag(&intent, "zap_id")?;
    if zap_id.len() < 32
        || !zap_id.len().is_multiple_of(2)
        || !zap_id
            .chars()
            .all(|character| character.is_ascii_hexdigit() && !character.is_ascii_uppercase())
    {
        return Err(WalletError::new(
            "invalid_zap",
            "zap intent has an invalid zap_id",
        ));
    }

    for name in ["e", "a", "k"] {
        matching_optional_tag(&event, &intent, name)?;
    }
    let target_event_id = optional_event_tag(&event, "e")?.map(str::to_string);
    let address_target = optional_event_tag(&event, "a")?;
    if target_event_id.is_some() && address_target.is_some() {
        return Err(WalletError::new(
            "invalid_zap",
            "zap cannot target both an event and an address",
        ));
    }

    recipient_offer(&offer_event, &recipient)?;
    if offer_event.created_at > intent.created_at {
        return Err(WalletError::new(
            "invalid_zap",
            "zap intent predates its offer announcement",
        ));
    }
    if optional_event_tag(&event, "P")?
        .is_some_and(|payer| payer.to_ascii_lowercase() != event.pubkey.to_hex())
    {
        return Err(WalletError::new(
            "invalid_zap",
            "zap payer tag does not match its signer",
        ));
    }

    Ok(WalletVerifiedZapEvent {
        event_id: event.id.to_hex(),
        amount: amount_msats / 1_000,
        comment: intent.content,
        intent_event_id: intent.id.to_hex(),
        recipient_pubkey: recipient,
        target_event_id,
    })
}

fn validate_canonical_offer(offer: &str) -> Result<(), WalletError> {
    if !offer.starts_with("lno1")
        || offer.chars().any(char::is_whitespace)
        || offer != offer.to_ascii_lowercase()
    {
        return Err(WalletError::new(
            "offer_invalid",
            "BOLT12 offer must be canonical lowercase lno1",
        ));
    }
    if !canonical_offer(offer) {
        return Err(WalletError::new(
            "offer_invalid",
            "BOLT12 offer is not canonically encoded",
        ));
    }
    Ok(())
}

fn random_zap_id() -> Result<String, WalletError> {
    let mut bytes = [0_u8; 16];
    getrandom::fill(&mut bytes)
        .map_err(|error| WalletError::unavailable(format!("generate zap id: {error}")))?;
    Ok(hex::encode(bytes))
}

fn signed_intent_with_runtime_quote(
    keys: &Keys,
    recipient: &WalletRecipientOffer,
    amount: u64,
    comment: &str,
    target_event_id: Option<&str>,
    target_event_kind: Option<u32>,
    runtime_quote_event_json: Option<&str>,
) -> Result<Event, WalletError> {
    if target_event_id.is_some() != target_event_kind.is_some() {
        return Err(WalletError::new(
            "invalid_zap",
            "event zaps require both a target event id and kind",
        ));
    }
    if let Some(event_id) = target_event_id {
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
    if let (Some(event_id), Some(event_kind)) = (target_event_id, target_event_kind) {
        let event_kind = event_kind.to_string();
        tags.push(tag(["e", event_id])?);
        tags.push(tag(["k", event_kind.as_str()])?);
    }
    if let Some(quote_event_json) = runtime_quote_event_json {
        tags.push(tag(["agent_runtime_quote", quote_event_json])?);
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

/// Durable checkpoints for a profile payment.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ZapAttemptState {
    /// Intent and recipient offer are persisted; no payment has started.
    Prepared,
    /// Payment may have reached Lexe and must be reconciled before retrying.
    Paying,
    /// Payment settled. A kind `9736` event carrying the temporary placeholder
    /// payer proof can now be published.
    #[serde(
        alias = "paid_awaiting_proof",
        alias = "publishing_placeholder",
        alias = "placeholder_published"
    )]
    PaidWithoutProof,
    /// The provider reported a terminal failure. A new user-confirmed attempt
    /// may use a new idempotency key.
    Failed,
}

impl ZapAttemptState {
    fn is_terminal(self) -> bool {
        matches!(self, Self::PaidWithoutProof | Self::Failed)
    }
}

/// Persisted data needed to resume one profile payment safely.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ZapAttempt {
    /// Persistence schema version.
    pub version: u8,
    /// UUID supplied by the UI and used as the durable attempt filename.
    pub idempotency_key: String,
    pub recipient_pubkey: String,
    /// Whole satoshis paid to the recipient (displayed as ₿ per BIP-177).
    pub amount: u64,
    pub comment: Option<String>,
    /// Event target frozen into the signed intent. Both fields are absent for
    /// profile zaps and present for message zaps.
    #[serde(default)]
    pub target_event_id: Option<String>,
    #[serde(default)]
    pub target_event_kind: Option<u32>,
    /// Canonical BOLT12 offer frozen before payment.
    pub offer: String,
    /// Exact recipient-signed offer event embedded by the intent.
    pub offer_event_json: String,
    /// Exact payer-signed, unbroadcast kind `9737` intent.
    pub intent_event_json: String,
    pub intent_event_id: String,
    /// Exact signed kind-24211 quote for an agent-runtime purchase.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runtime_quote_event_json: Option<String>,
    /// Intent reference sent in the BOLT12 payer note for reconciliation.
    pub payer_note: String,
    pub state: ZapAttemptState,
    pub payment: Option<WalletPaymentResult>,
    /// Exact signed kind `9736` event retained for idempotent relay retries.
    #[serde(default)]
    pub proof_event_json: Option<String>,
    /// Whether the persisted proof event was accepted by the active relay.
    #[serde(default)]
    pub proof_published: bool,
    #[serde(default)]
    pub updated_at_ms: u64,
}

impl ZapAttempt {
    pub fn prepare(
        idempotency_key: String,
        recipient: WalletRecipientOffer,
        amount: u64,
        comment: Option<String>,
        target_event_id: Option<String>,
        target_event_kind: Option<u32>,
        keys: &Keys,
    ) -> Result<Self, WalletError> {
        let target = match (target_event_id, target_event_kind) {
            (Some(id), Some(kind)) => Some((id, kind)),
            (None, None) => None,
            _ => {
                return Err(WalletError::new(
                    "zap_target_invalid",
                    "message zap target id and kind must be provided together",
                ))
            }
        };
        Self::prepare_inner(
            idempotency_key,
            recipient,
            amount,
            comment,
            target,
            None,
            keys,
        )
    }

    /// Prepare a runtime-zap attempt whose intent embeds the exact signed quote.
    pub fn prepare_agent_runtime(
        idempotency_key: String,
        recipient: WalletRecipientOffer,
        amount: u64,
        quote_event_json: String,
        keys: &Keys,
    ) -> Result<Self, WalletError> {
        Self::prepare_inner(
            idempotency_key,
            recipient,
            amount,
            None,
            None,
            Some(quote_event_json),
            keys,
        )
    }

    fn prepare_inner(
        idempotency_key: String,
        recipient: WalletRecipientOffer,
        amount: u64,
        comment: Option<String>,
        target: Option<(String, u32)>,
        runtime_quote_event_json: Option<String>,
        keys: &Keys,
    ) -> Result<Self, WalletError> {
        let comment = comment
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty());
        let intent = signed_intent_with_runtime_quote(
            keys,
            &recipient,
            amount,
            comment.as_deref().unwrap_or_default(),
            target.as_ref().map(|(id, _)| id.as_str()),
            target.as_ref().map(|(_, kind)| *kind),
            runtime_quote_event_json.as_deref(),
        )?;
        let intent_event_id = intent.id.to_hex();
        let (target_event_id, target_event_kind) = target
            .map(|(id, kind)| (Some(id), Some(kind)))
            .unwrap_or((None, None));
        Ok(Self {
            version: 4,
            idempotency_key,
            recipient_pubkey: recipient.recipient_pubkey,
            amount,
            comment,
            target_event_id,
            target_event_kind,
            offer: recipient.offer,
            offer_event_json: recipient.offer_event_json,
            intent_event_json: intent.as_json(),
            runtime_quote_event_json,
            payer_note: format!("nostr:nipB1:{intent_event_id}"),
            intent_event_id,
            state: ZapAttemptState::Prepared,
            payment: None,
            proof_event_json: None,
            proof_published: false,
            updated_at_ms: now_ms(),
        })
    }

    pub fn touch(&mut self) {
        self.updated_at_ms = now_ms();
    }

    pub fn result(&self) -> Option<WalletProfileZapResult> {
        Some(WalletProfileZapResult {
            payment: self.payment.clone()?,
            intent_event_id: self.intent_event_id.clone(),
            proof_published: self.proof_published,
        })
    }

    /// Decode the exact signed proof event retained for publication retries.
    pub fn proof_event(&self) -> Result<Option<Event>, WalletError> {
        self.proof_event_json
            .as_deref()
            .map(|json| {
                Event::from_json(json).map_err(|error| {
                    WalletError::unavailable(format!("decode persisted zap proof event: {error}"))
                })
            })
            .transpose()
    }

    fn build_placeholder_proof_event(
        &self,
        keys: &Keys,
        channel_id: Option<&str>,
    ) -> Result<Event, WalletError> {
        if self.state != ZapAttemptState::PaidWithoutProof
            || self.payment.as_ref().map(|payment| payment.status.as_str()) != Some("completed")
        {
            return Err(WalletError::unavailable(
                "placeholder zap proof requires a settled payment",
            ));
        }
        let intent = Event::from_json(&self.intent_event_json)
            .map_err(|error| WalletError::new("invalid_zap", error.to_string()))?;
        let mut tags = intent
            .tags
            .iter()
            .filter(|tag| tag.as_slice().first().map(String::as_str) != Some("zap_id"))
            .cloned()
            .collect::<Vec<_>>();
        tags.push(tag(["description", self.intent_event_json.as_str()])?);
        tags.push(tag(["P", keys.public_key().to_hex().as_str()])?);
        tags.push(tag(["proof", PLACEHOLDER_PAYER_PROOF])?);
        if let Some(channel_id) = channel_id {
            tags.push(tag(["h", channel_id])?);
        }
        EventBuilder::new(Kind::Custom(KIND_BOLT12_ZAP as u16), intent.content)
            .tags(tags)
            .sign_with_keys(keys)
            .map_err(|error| {
                WalletError::new(
                    "relay_publish_failed",
                    format!("sign placeholder zap proof: {error}"),
                )
            })
    }

    fn draft(&self) -> WalletProfileZapDraft {
        WalletProfileZapDraft {
            recipient_pubkey: self.recipient_pubkey.clone(),
            amount: self.amount,
            comment: self.comment.clone(),
            idempotency_key: self.idempotency_key.clone(),
            target_event_id: self.target_event_id.clone(),
            target_event_kind: self.target_event_kind,
        }
    }

    fn abstract_state(&self) -> WalletAbstractState {
        let status = match self.state {
            ZapAttemptState::Prepared => WalletAttemptStatus::ProfilePrepared,
            ZapAttemptState::Paying => WalletAttemptStatus::ProfilePaying,
            ZapAttemptState::PaidWithoutProof => WalletAttemptStatus::ProfilePaidWithoutProof,
            ZapAttemptState::Failed => WalletAttemptStatus::ProfileFailed,
        };
        WalletAbstractState {
            status,
            payment_recorded: self.payment.is_some(),
        }
    }
}

/// Atomic, identity-scoped persistence for profile-payment checkpoints.
pub struct ZapAttemptStore {
    directory: PathBuf,
    payer_pubkey: String,
}

impl ZapAttemptStore {
    pub fn new(app_data_dir: &Path, payer_pubkey: &str) -> Self {
        Self {
            directory: app_data_dir
                .join("wallet")
                .join("zap-attempts")
                .join(payer_pubkey),
            payer_pubkey: payer_pubkey.to_string(),
        }
    }

    fn path(&self, idempotency_key: &str) -> Result<PathBuf, WalletError> {
        let id = Uuid::parse_str(idempotency_key).map_err(|_| {
            WalletError::new(
                "invalid_idempotency_key",
                "profile payment idempotency key must be a UUID",
            )
        })?;
        Ok(self.directory.join(format!("{id}.json")))
    }

    pub fn load(&self, idempotency_key: &str) -> Result<Option<ZapAttempt>, WalletError> {
        let path = self.path(idempotency_key)?;
        match std::fs::read(&path) {
            Ok(bytes) => serde_json::from_slice(&bytes)
                .map(Some)
                .map_err(|error| WalletError::unavailable(format!("read zap attempt: {error}"))),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(WalletError::unavailable(format!(
                "open zap attempt: {error}"
            ))),
        }
    }

    fn save(&self, attempt: &mut ZapAttempt) -> Result<(), WalletError> {
        attempt.touch();
        std::fs::create_dir_all(&self.directory)
            .map_err(|error| WalletError::unavailable(format!("create zap store: {error}")))?;
        let path = self.path(&attempt.idempotency_key)?;
        let bytes = serde_json::to_vec_pretty(attempt)
            .map_err(|error| WalletError::unavailable(format!("encode zap attempt: {error}")))?;
        let mut file = AtomicWriteFile::open(path)
            .map_err(|error| WalletError::unavailable(format!("open zap attempt: {error}")))?;
        file.write_all(&bytes)
            .map_err(|error| WalletError::unavailable(format!("write zap attempt: {error}")))?;
        file.commit()
            .map_err(|error| WalletError::unavailable(format!("commit zap attempt: {error}")))
    }

    /// Persist a new profile attempt and emit `PrepareProfile`.
    pub fn save_prepared(&self, attempt: &mut ZapAttempt) -> Result<(), WalletError> {
        if attempt.state != ZapAttemptState::Prepared || attempt.payment.is_some() {
            return Err(WalletError::unavailable(
                "new profile payment is not in the prepared state",
            ));
        }
        self.save(attempt)?;
        conformance::record(
            &self.payer_pubkey,
            conformance::WalletAttemptKind::ProfileZap,
            &attempt.idempotency_key,
            WalletTraceAction::PrepareProfile,
            WalletAbstractState::absent(),
            attempt.abstract_state(),
        );
        Ok(())
    }

    /// Durably enter `Paying` before the only provider send call.
    pub fn begin_dispatch(&self, attempt: &mut ZapAttempt) -> Result<(), WalletError> {
        let before = attempt.abstract_state();
        if attempt.state != ZapAttemptState::Prepared || attempt.payment.is_some() {
            return Err(WalletError::unavailable(
                "profile payment cannot dispatch from its current state",
            ));
        }
        attempt.state = ZapAttemptState::Paying;
        self.save(attempt)?;
        conformance::record(
            &self.payer_pubkey,
            conformance::WalletAttemptKind::ProfileZap,
            &attempt.idempotency_key,
            WalletTraceAction::BeginDispatch,
            before,
            attempt.abstract_state(),
        );
        Ok(())
    }

    /// Record that this invocation chose provider reconciliation, not send.
    pub fn record_reconcile(&self, attempt: &ZapAttempt) -> Result<(), WalletError> {
        if attempt.state != ZapAttemptState::Paying {
            return Err(WalletError::unavailable(
                "profile payment cannot reconcile from its current state",
            ));
        }
        let state = attempt.abstract_state();
        conformance::record(
            &self.payer_pubkey,
            conformance::WalletAttemptKind::ProfileZap,
            &attempt.idempotency_key,
            WalletTraceAction::Reconcile,
            state,
            state,
        );
        Ok(())
    }

    /// Persist a provider result and emit the matching modeled transition.
    pub fn record_payment(
        &self,
        attempt: &mut ZapAttempt,
        payment: WalletPaymentResult,
    ) -> Result<(), WalletError> {
        let before = attempt.abstract_state();
        if attempt.state != ZapAttemptState::Paying {
            return Err(WalletError::unavailable(
                "profile payment cannot record payment from its current state",
            ));
        }
        let action = match payment.status.as_str() {
            "completed" => {
                attempt.state = ZapAttemptState::PaidWithoutProof;
                WalletTraceAction::RecordPaidWithoutProof
            }
            "failed" => {
                attempt.state = ZapAttemptState::Failed;
                WalletTraceAction::RecordFailed {
                    payment_recorded: true,
                }
            }
            _ => WalletTraceAction::RecordPending,
        };
        attempt.payment = Some(payment);
        self.save(attempt)?;
        conformance::record(
            &self.payer_pubkey,
            conformance::WalletAttemptKind::ProfileZap,
            &attempt.idempotency_key,
            action,
            before,
            attempt.abstract_state(),
        );
        Ok(())
    }

    /// Persist the exact signed placeholder proof before relay publication.
    pub fn prepare_placeholder_proof(
        &self,
        attempt: &mut ZapAttempt,
        keys: &Keys,
        channel_id: Option<&str>,
    ) -> Result<Event, WalletError> {
        if let Some(event) = attempt.proof_event()? {
            return Ok(event);
        }
        let event = attempt.build_placeholder_proof_event(keys, channel_id)?;
        attempt.proof_event_json = Some(event.as_json());
        self.save(attempt)?;
        Ok(event)
    }

    /// Persist successful publication to the active relay.
    pub fn mark_proof_published(&self, attempt: &mut ZapAttempt) -> Result<(), WalletError> {
        if attempt.proof_event_json.is_none() {
            return Err(WalletError::unavailable(
                "cannot mark a missing zap proof event as published",
            ));
        }
        attempt.proof_published = true;
        self.save(attempt)
    }

    /// Fail an expired reconciliation without inventing a provider result.
    pub fn fail_reconciliation(&self, attempt: &mut ZapAttempt) -> Result<(), WalletError> {
        let before = attempt.abstract_state();
        if attempt.state != ZapAttemptState::Paying {
            return Err(WalletError::unavailable(
                "profile reconciliation cannot expire from its current state",
            ));
        }
        let payment_recorded = attempt.payment.is_some();
        attempt.state = ZapAttemptState::Failed;
        self.save(attempt)?;
        conformance::record(
            &self.payer_pubkey,
            conformance::WalletAttemptKind::ProfileZap,
            &attempt.idempotency_key,
            WalletTraceAction::RecordFailed { payment_recorded },
            before,
            attempt.abstract_state(),
        );
        Ok(())
    }

    /// Emit a no-side-effect terminal replay decision.
    pub fn record_terminal_reuse(&self, attempt: &ZapAttempt) {
        let state = attempt.abstract_state();
        conformance::record(
            &self.payer_pubkey,
            conformance::WalletAttemptKind::ProfileZap,
            &attempt.idempotency_key,
            WalletTraceAction::ReuseTerminal,
            state,
            state,
        );
    }

    /// Emit rejection of different details under an existing idempotency key.
    pub fn record_conflict(&self, attempt: &ZapAttempt) {
        let state = attempt.abstract_state();
        conformance::record(
            &self.payer_pubkey,
            conformance::WalletAttemptKind::ProfileZap,
            &attempt.idempotency_key,
            WalletTraceAction::RejectConflict,
            state,
            state,
        );
    }

    pub fn pending_for_recipient(
        &self,
        recipient_pubkey: &str,
        target_event_id: Option<&str>,
    ) -> Result<Option<WalletProfileZapDraft>, WalletError> {
        let entries = match std::fs::read_dir(&self.directory) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => {
                return Err(WalletError::unavailable(format!(
                    "read zap attempt directory: {error}"
                )))
            }
        };
        let mut latest: Option<ZapAttempt> = None;
        for entry in entries.flatten() {
            let Ok(bytes) = std::fs::read(entry.path()) else {
                continue;
            };
            let Ok(attempt) = serde_json::from_slice::<ZapAttempt>(&bytes) else {
                continue;
            };
            let needs_reconciliation = matches!(
                attempt.state,
                ZapAttemptState::Prepared | ZapAttemptState::Paying
            ) || (attempt.state == ZapAttemptState::PaidWithoutProof
                && !attempt.proof_published);
            if attempt.recipient_pubkey == recipient_pubkey
                && attempt.target_event_id.as_deref() == target_event_id
                && needs_reconciliation
                && latest
                    .as_ref()
                    .is_none_or(|current| attempt.updated_at_ms > current.updated_at_ms)
            {
                latest = Some(attempt);
            }
        }
        Ok(latest.map(|attempt| attempt.draft()))
    }

    /// List settled event-targeted payments for local fallback rendering.
    ///
    /// Keep published attempts in this list. Relay acceptance and renderer
    /// hydration are separate steps, so hiding a receipt as soon as the relay
    /// accepts its proof creates a gap where neither the local receipt nor the
    /// public zap is visible. The renderer deduplicates these receipts by the
    /// intent event id once the matching public proof is hydrated.
    pub fn settled_message_zaps(&self) -> Result<Vec<WalletPlaceholderMessageZap>, WalletError> {
        let entries = match std::fs::read_dir(&self.directory) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => {
                return Err(WalletError::unavailable(format!(
                    "read zap attempt directory: {error}"
                )))
            }
        };
        let mut receipts = entries
            .flatten()
            .filter_map(|entry| std::fs::read(entry.path()).ok())
            .filter_map(|bytes| serde_json::from_slice::<ZapAttempt>(&bytes).ok())
            .filter_map(|attempt| {
                let target_event_id = attempt.target_event_id?;
                let payment = attempt.payment?;
                (attempt.state == ZapAttemptState::PaidWithoutProof
                    && payment.status == "completed")
                    .then(|| WalletPlaceholderMessageZap {
                        intent_event_id: attempt.intent_event_id,
                        target_event_id,
                        recipient_pubkey: attempt.recipient_pubkey,
                        amount: payment.amount.unwrap_or(attempt.amount),
                        comment: attempt.comment,
                        settled_at_ms: payment.finalized_at_ms.unwrap_or(payment.created_at_ms),
                    })
            })
            .collect::<Vec<_>>();
        receipts.sort_by_key(|receipt| receipt.settled_at_ms);
        Ok(receipts)
    }

    /// Remove terminal checkpoints after the documented 90-day retention
    /// period. Incomplete attempts are retained until reconciled.
    pub fn prune(&self) -> Result<(), WalletError> {
        let entries = match std::fs::read_dir(&self.directory) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(error) => {
                return Err(WalletError::unavailable(format!(
                    "read zap attempt directory: {error}"
                )))
            }
        };
        let cutoff = now_ms().saturating_sub(TERMINAL_ATTEMPT_RETENTION_MS);
        for entry in entries.flatten() {
            let path = entry.path();
            let Ok(bytes) = std::fs::read(&path) else {
                continue;
            };
            let Ok(attempt) = serde_json::from_slice::<ZapAttempt>(&bytes) else {
                continue;
            };
            if attempt.state.is_terminal() && attempt.updated_at_ms < cutoff {
                if let Err(error) = std::fs::remove_file(&path) {
                    tracing::warn!(path = %path.display(), error = %error, "prune wallet attempt");
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
#[path = "zap/runtime_tests.rs"]
mod runtime_tests;

#[cfg(test)]
mod tests {
    use super::*;

    use crate::wallet::VALID_OFFER;

    fn recipient(keys: &Keys) -> WalletRecipientOffer {
        let event = EventBuilder::new(Kind::Custom(KIND_BOLT12_OFFER as u16), "")
            .tag(Tag::parse(["offer", VALID_OFFER]).unwrap())
            .sign_with_keys(keys)
            .unwrap();
        recipient_offer(&event, &keys.public_key().to_hex()).unwrap()
    }

    fn tagged_zap_with_offer(offer: &str) -> (Event, Event) {
        let payer = Keys::generate();
        let recipient = Keys::generate();
        let offer_event = EventBuilder::new(Kind::Custom(KIND_BOLT12_OFFER as u16), "")
            .tag(Tag::parse(["offer", offer]).unwrap())
            .sign_with_keys(&recipient)
            .unwrap();
        let target_event_id = "ab".repeat(32);
        let intent = EventBuilder::new(Kind::Custom(KIND_BOLT12_ZAP_INTENT as u16), "nice work")
            .tags([
                Tag::parse(["p", recipient.public_key().to_hex().as_str()]).unwrap(),
                Tag::parse(["amount", "21000"]).unwrap(),
                Tag::parse(["offer_event", offer_event.as_json().as_str()]).unwrap(),
                Tag::parse(["zap_id", "00112233445566778899aabbccddeeff"]).unwrap(),
                Tag::parse(["e", target_event_id.as_str()]).unwrap(),
                Tag::parse(["k", "40002"]).unwrap(),
            ])
            .sign_with_keys(&payer)
            .unwrap();
        let mut proof_tags = intent
            .tags
            .iter()
            .filter(|tag| tag.as_slice().first().map(String::as_str) != Some("zap_id"))
            .cloned()
            .collect::<Vec<_>>();
        proof_tags.extend([
            Tag::parse(["description", intent.as_json().as_str()]).unwrap(),
            Tag::parse(["P", payer.public_key().to_hex().as_str()]).unwrap(),
            Tag::parse(["proof", PLACEHOLDER_PAYER_PROOF]).unwrap(),
        ]);
        let zap = EventBuilder::new(Kind::Custom(KIND_BOLT12_ZAP as u16), "nice work")
            .tags(proof_tags)
            .sign_with_keys(&payer)
            .unwrap();
        (intent, zap)
    }

    #[test]
    fn intent_matches_protocol_shape_and_uses_nip_b1_note() {
        let payer = Keys::generate();
        let recipient_keys = Keys::generate();
        let attempt = ZapAttempt::prepare(
            Uuid::new_v4().to_string(),
            recipient(&recipient_keys),
            21,
            Some("great work".to_string()),
            None,
            None,
            &payer,
        )
        .unwrap();
        let intent = Event::from_json(&attempt.intent_event_json).unwrap();
        assert_eq!(intent.kind, Kind::Custom(KIND_BOLT12_ZAP_INTENT as u16));
        assert_eq!(intent.content, "great work");
        assert!(intent
            .tags
            .iter()
            .any(|tag| tag.as_slice() == ["amount", "21000"]));
        assert_eq!(
            attempt.payer_note,
            format!("nostr:nipB1:{}", intent.id.to_hex())
        );
        let zap_id = intent
            .tags
            .iter()
            .find_map(|tag| {
                let parts = tag.as_slice();
                (parts.first().map(String::as_str) == Some("zap_id"))
                    .then(|| parts.get(1))
                    .flatten()
            })
            .unwrap();
        assert_eq!(zap_id.len(), 32);
        assert!(zap_id
            .chars()
            .all(|character| character.is_ascii_hexdigit()));
        assert_eq!(zap_id, &zap_id.to_ascii_lowercase());
    }

    #[test]
    fn event_intent_binds_message_id_and_kind() {
        let payer = Keys::generate();
        let recipient_keys = Keys::generate();
        let target_event_id = "ab".repeat(32);
        let attempt = ZapAttempt::prepare(
            Uuid::new_v4().to_string(),
            recipient(&recipient_keys),
            21,
            None,
            Some(target_event_id.clone()),
            Some(40_002),
            &payer,
        )
        .unwrap();
        let intent = Event::from_json(&attempt.intent_event_json).unwrap();
        assert!(intent
            .tags
            .iter()
            .any(|tag| tag.as_slice() == ["e", target_event_id.as_str()]));
        assert!(intent
            .tags
            .iter()
            .any(|tag| tag.as_slice() == ["k", "40002"]));
    }

    #[test]
    fn parses_only_canonical_lowercase_offers() {
        assert!(build_offer_announcement(VALID_OFFER).is_ok());
        assert!(build_offer_announcement(&VALID_OFFER.to_ascii_uppercase()).is_err());
        assert!(build_offer_announcement(&VALID_OFFER[..VALID_OFFER.len() - 1]).is_err());
        assert!(build_offer_announcement(&format!("{VALID_OFFER} ")).is_err());
    }

    #[test]
    fn parses_tagged_zap_with_rust_lightning_offer() {
        let (intent, zap) = tagged_zap_with_offer(VALID_OFFER);
        let raw = serde_json::from_str(&zap.as_json()).unwrap();
        let parsed = parse_tagged_zap_event(&raw).unwrap();
        assert_eq!(parsed.event_id, zap.id.to_hex());
        assert_eq!(parsed.intent_event_id, intent.id.to_hex());
        assert_eq!(parsed.amount, 21);
        assert_eq!(parsed.comment, "nice work");
        assert_eq!(parsed.target_event_id, Some("ab".repeat(32)));
    }

    #[test]
    fn tagged_zap_rejects_offer_that_rust_lightning_cannot_parse() {
        let (_, zap) = tagged_zap_with_offer(&VALID_OFFER[..VALID_OFFER.len() - 1]);
        let raw = serde_json::from_str(&zap.as_json()).unwrap();
        assert_eq!(
            parse_tagged_zap_event(&raw).unwrap_err().code,
            "offer_invalid"
        );
    }

    #[test]
    fn rejects_announcement_when_any_offer_is_not_canonical() {
        let keys = Keys::generate();
        let event = EventBuilder::new(Kind::Custom(KIND_BOLT12_OFFER as u16), "")
            .tags([
                Tag::parse(["offer", VALID_OFFER]).unwrap(),
                Tag::parse(["offer", &VALID_OFFER.to_ascii_uppercase()]).unwrap(),
            ])
            .sign_with_keys(&keys)
            .unwrap();
        assert_eq!(
            recipient_offer(&event, &keys.public_key().to_hex())
                .unwrap_err()
                .code,
            "offer_invalid"
        );
    }

    #[test]
    fn rejects_zero_or_overflowing_amount() {
        assert!(amount_msats(0).is_err());
        assert!(amount_msats(u64::MAX).is_err());
    }

    #[test]
    fn attempt_store_round_trips_and_restores_pending_draft() {
        let temp = tempfile::tempdir().unwrap();
        let payer = Keys::generate();
        let recipient_keys = Keys::generate();
        let mut attempt = ZapAttempt::prepare(
            Uuid::new_v4().to_string(),
            recipient(&recipient_keys),
            21,
            None,
            None,
            None,
            &payer,
        )
        .unwrap();
        let store = ZapAttemptStore::new(temp.path(), &payer.public_key().to_hex());
        store.save_prepared(&mut attempt).unwrap();
        assert_eq!(
            store.load(&attempt.idempotency_key).unwrap(),
            Some(attempt.clone())
        );
        assert_eq!(
            store
                .pending_for_recipient(&attempt.recipient_pubkey, None)
                .unwrap()
                .unwrap()
                .idempotency_key,
            attempt.idempotency_key
        );
    }

    #[test]
    fn settled_message_zaps_restore_local_placeholder_receipts() {
        let temp = tempfile::tempdir().unwrap();
        let payer = Keys::generate();
        let recipient_keys = Keys::generate();
        let target_event_id = "ab".repeat(32);
        let mut attempt = ZapAttempt::prepare(
            Uuid::new_v4().to_string(),
            recipient(&recipient_keys),
            21,
            Some("great work".to_string()),
            Some(target_event_id.clone()),
            Some(40_002),
            &payer,
        )
        .unwrap();
        let store = ZapAttemptStore::new(temp.path(), &payer.public_key().to_hex());
        store.save_prepared(&mut attempt).unwrap();
        store.begin_dispatch(&mut attempt).unwrap();
        store
            .record_payment(
                &mut attempt,
                WalletPaymentResult {
                    payment_id: "payment".to_string(),
                    status: "completed".to_string(),
                    status_message: String::new(),
                    amount: Some(21),
                    fees: 0,
                    created_at_ms: 100,
                    finalized_at_ms: Some(200),
                },
            )
            .unwrap();

        let receipts = store.settled_message_zaps().unwrap();
        assert_eq!(receipts.len(), 1);
        assert_eq!(receipts[0].intent_event_id, attempt.intent_event_id);
        assert_eq!(receipts[0].target_event_id, target_event_id);
        assert_eq!(receipts[0].amount, 21);
        assert_eq!(receipts[0].settled_at_ms, 200);
        assert_eq!(
            store
                .pending_for_recipient(&attempt.recipient_pubkey, Some(&target_event_id))
                .unwrap()
                .unwrap()
                .idempotency_key,
            attempt.idempotency_key
        );

        let proof = store
            .prepare_placeholder_proof(&mut attempt, &payer, Some("channel-id"))
            .unwrap();
        assert_eq!(proof.kind, Kind::Custom(KIND_BOLT12_ZAP as u16));
        assert!(proof
            .tags
            .iter()
            .any(|tag| tag.as_slice() == ["proof", PLACEHOLDER_PAYER_PROOF]));
        assert!(proof
            .tags
            .iter()
            .any(|tag| tag.as_slice() == ["h", "channel-id"]));
        let persisted = store
            .prepare_placeholder_proof(&mut attempt, &payer, None)
            .unwrap();
        assert_eq!(persisted.id, proof.id);
        store.mark_proof_published(&mut attempt).unwrap();
        let published_receipts = store.settled_message_zaps().unwrap();
        assert_eq!(published_receipts.len(), 1);
        assert_eq!(
            published_receipts[0].intent_event_id,
            attempt.intent_event_id
        );
        assert!(store
            .pending_for_recipient(&attempt.recipient_pubkey, Some(&target_event_id))
            .unwrap()
            .is_none());
        assert!(attempt.result().unwrap().proof_published);
    }
}

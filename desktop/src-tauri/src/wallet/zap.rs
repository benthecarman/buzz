use std::{
    io::Write,
    path::{Path, PathBuf},
    str::FromStr,
    time::{SystemTime, UNIX_EPOCH},
};

use buzz_conformance_pkg::wallet::{WalletAbstractState, WalletAttemptStatus, WalletTraceAction};
use buzz_core_pkg::kind::{KIND_BOLT12_OFFER, KIND_BOLT12_ZAP, KIND_BOLT12_ZAP_INTENT};
use lightning_payer_proof::{verify, Offer};
use nostr::{Event, EventBuilder, JsonUtil, Keys, Kind, Tag};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::conformance;
use super::models::{
    WalletError, WalletPaymentResult, WalletPaymentStatus, WalletProfileZapDraft,
    WalletProfileZapResult, WalletRecipientOffer,
};

const TERMINAL_ATTEMPT_RETENTION_MS: u64 = 90 * 24 * 60 * 60 * 1_000;
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
        .filter(|offer| validate_canonical_offer(offer).is_ok())
        .collect::<Vec<_>>();
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
    let parsed = Offer::from_str(offer)
        .map_err(|_| WalletError::new("offer_invalid", "BOLT12 offer cannot be decoded"))?;
    if parsed.to_string() != offer {
        return Err(WalletError::new(
            "offer_invalid",
            "BOLT12 offer is not canonically encoded",
        ));
    }
    Ok(())
}

mod intent;
use intent::signed_intent;
pub use intent::ZapTarget;

/// Durable checkpoints for a profile payment.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ZapAttemptState {
    /// Intent and recipient offer are persisted; no payment has started.
    Prepared,
    /// Payment may have reached Lexe and must be reconciled before retrying.
    Paying,
    /// Payment settled. A kind `9736` event carrying the payer proof can now be
    /// published.
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
    /// Channel copied into a channel-scoped zap.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub channel_id: Option<String>,
    /// Existing hosted-agent lease selected by this signed intent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lease_id: Option<String>,
    /// Intent reference sent in the BOLT12 payer note for reconciliation.
    pub payer_note: String,
    /// Relay where the recipient offer was resolved and the proof belongs.
    /// Legacy attempts omit this and remain manual-only so a community switch
    /// cannot publish their proof to the wrong relay.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub relay_url: Option<String>,
    pub state: ZapAttemptState,
    pub payment: Option<WalletPaymentResult>,
    /// Exact signed kind `9736` event retained for idempotent relay retries.
    #[serde(default)]
    pub proof_event_json: Option<String>,
    /// Whether the persisted proof event was accepted by the active relay.
    #[serde(default)]
    pub proof_published: bool,
    /// Legacy flag retained for version 5 records; proof recovery ignores it.
    #[serde(default)]
    pub proof_retry_abandoned: bool,
    #[serde(default)]
    pub updated_at_ms: u64,
}

impl ZapAttempt {
    pub fn prepare(
        idempotency_key: String,
        recipient: WalletRecipientOffer,
        amount: u64,
        comment: Option<String>,
        target: ZapTarget,
        keys: &Keys,
    ) -> Result<Self, WalletError> {
        let comment = comment
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty());
        let intent = signed_intent(
            keys,
            &recipient,
            amount,
            comment.as_deref().unwrap_or_default(),
            &target,
        )?;
        let intent_event_id = intent.id.to_hex();
        Ok(Self {
            version: 6,
            idempotency_key,
            recipient_pubkey: recipient.recipient_pubkey,
            amount,
            comment,
            target_event_id: target.event_id,
            target_event_kind: target.event_kind,
            offer: recipient.offer,
            offer_event_json: recipient.offer_event_json,
            intent_event_json: intent.as_json(),
            channel_id: target.channel_id,
            lease_id: target.lease_id,
            payer_note: format!("nostr:nipB1:{intent_event_id}"),
            intent_event_id,
            relay_url: None,
            state: ZapAttemptState::Prepared,
            payment: None,
            proof_event_json: None,
            proof_published: false,
            proof_retry_abandoned: false,
            updated_at_ms: now_ms(),
        })
    }

    pub fn touch(&mut self) {
        self.updated_at_ms = now_ms();
    }

    pub fn result(&self) -> Option<WalletProfileZapResult> {
        let proof = self.proof_event().ok().flatten();
        Some(WalletProfileZapResult {
            payment: self.payment.clone()?,
            intent_event_id: self.intent_event_id.clone(),
            proof_event_id: proof.as_ref().map(|event| event.id.to_hex()),
            proof_created_at_seconds: proof.map(|event| event.created_at.as_secs()),
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

    fn build_proof_event(
        &self,
        keys: &Keys,
        channel_id: Option<&str>,
    ) -> Result<Event, WalletError> {
        if self.state != ZapAttemptState::PaidWithoutProof
            || self.payment.as_ref().map(|payment| payment.status)
                != Some(WalletPaymentStatus::Completed)
        {
            return Err(WalletError::unavailable(
                "zap proof requires a settled payment",
            ));
        }
        let intent = Event::from_json(&self.intent_event_json)
            .map_err(|error| WalletError::new("invalid_zap", error.to_string()))?;
        let intent_channels = intent
            .tags
            .iter()
            .filter_map(|tag| {
                let parts = tag.as_slice();
                (parts.first().map(String::as_str) == Some("h"))
                    .then(|| parts.get(1).map(String::as_str))
                    .flatten()
            })
            .collect::<Vec<_>>();
        if intent_channels.len() > 1
            || intent_channels
                .first()
                .is_some_and(|intent_channel| Some(*intent_channel) != channel_id)
        {
            return Err(WalletError::new(
                "invalid_zap",
                "The channel must be signed into the zap intent",
            ));
        }
        let mut tags = intent
            .tags
            .iter()
            .filter(|tag| tag.as_slice().first().map(String::as_str) != Some("zap_id"))
            .cloned()
            .collect::<Vec<_>>();
        let intent_json = intent.as_json();
        tags.push(tag(["description", intent_json.as_str()])?);
        tags.push(tag(["P", keys.public_key().to_hex().as_str()])?);
        let payer_proof = self
            .payment
            .as_ref()
            .and_then(|payment| payment.payer_proof.as_deref())
            .ok_or_else(|| {
                WalletError::new(
                    "payer_proof_unavailable",
                    "The settled BOLT12 payment has no payer proof",
                )
            })?;
        tags.push(tag(["proof", payer_proof])?);
        verify(payer_proof).map_err(|error| {
            WalletError::new(
                "payer_proof_invalid",
                format!("The settled BOLT12 payer proof is invalid: {error}"),
            )
        })?;
        EventBuilder::new(Kind::Custom(KIND_BOLT12_ZAP as u16), intent.content)
            .tags(tags)
            .sign_with_keys(keys)
            .map_err(|error| {
                WalletError::new("relay_publish_failed", format!("sign zap proof: {error}"))
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
            channel_id: self.channel_id.clone(),
            lease_id: self.lease_id.clone(),
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
        let filename = Uuid::parse_str(idempotency_key)
            .map(|id| id.to_string())
            .map_err(|_| {
                WalletError::new(
                    "invalid_idempotency_key",
                    "profile payment idempotency key must be a UUID",
                )
            })?;
        Ok(self.directory.join(format!("{filename}.json")))
    }

    fn stored_attempts(&self) -> Result<Vec<ZapAttempt>, WalletError> {
        let entries = match std::fs::read_dir(&self.directory) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => {
                return Err(WalletError::unavailable(format!(
                    "read zap attempt directory: {error}"
                )))
            }
        };
        let mut attempts = Vec::new();
        for entry in entries {
            let entry = match entry {
                Ok(entry) => entry,
                Err(error) => {
                    tracing::warn!(error = %error, "skip unreadable profile zap attempt entry");
                    continue;
                }
            };
            let path = entry.path();
            if path.extension().and_then(|value| value.to_str()) != Some("json") {
                continue;
            }
            let Some(key) = path.file_stem().and_then(|value| value.to_str()) else {
                tracing::warn!(path = %path.display(), "skip unnamed profile zap attempt");
                continue;
            };
            match self.load(key) {
                Ok(Some(attempt)) => attempts.push(attempt),
                Ok(None) => {}
                Err(error) => {
                    tracing::warn!(
                        path = %path.display(),
                        error = %error.message,
                        "skip invalid profile zap attempt"
                    );
                }
            }
        }
        Ok(attempts)
    }

    pub fn load(&self, idempotency_key: &str) -> Result<Option<ZapAttempt>, WalletError> {
        let path = self.path(idempotency_key)?;
        match std::fs::read(&path) {
            Ok(bytes) => {
                let attempt = serde_json::from_slice(&bytes).map_err(|error| {
                    WalletError::unavailable(format!("read zap attempt: {error}"))
                })?;
                self.validate_loaded_attempt(idempotency_key, &attempt)?;
                Ok(Some(attempt))
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(WalletError::unavailable(format!(
                "open zap attempt: {error}"
            ))),
        }
    }

    fn validate_loaded_attempt(
        &self,
        attempt_key: &str,
        attempt: &ZapAttempt,
    ) -> Result<(), WalletError> {
        let intent = Event::from_json(&attempt.intent_event_json)
            .map_err(|error| WalletError::new("invalid_payment_attempt", error.to_string()))?;
        intent
            .verify()
            .map_err(|error| WalletError::new("invalid_payment_attempt", error.to_string()))?;
        let target_kind = attempt.target_event_kind.map(|kind| kind.to_string());
        let expected_kind = if attempt.version >= 6 || attempt.channel_id.is_none() {
            target_kind.as_deref()
        } else {
            None
        };
        if attempt_key != attempt.idempotency_key
            || intent.kind != Kind::Custom(KIND_BOLT12_ZAP_INTENT as u16)
            || intent.id.to_hex() != attempt.intent_event_id
            || intent.pubkey.to_hex() != self.payer_pubkey
            || intent.content != attempt.comment.as_deref().unwrap_or_default()
            || exact_event_tag(&intent, "p")? != attempt.recipient_pubkey
            || exact_event_tag(&intent, "amount")? != amount_msats(attempt.amount)?
            || exact_event_tag(&intent, "offer_event")? != attempt.offer_event_json
            || optional_event_tag(&intent, "e")? != attempt.target_event_id.as_deref()
            || optional_event_tag(&intent, "k")? != expected_kind
            || optional_event_tag(&intent, "h")? != attempt.channel_id.as_deref()
            || optional_event_tag(&intent, "lease")? != attempt.lease_id.as_deref()
            || optional_event_tag(&intent, "a")?.is_some()
            || attempt.payer_note != format!("nostr:nipB1:{}", intent.id.to_hex())
        {
            return Err(WalletError::new(
                "invalid_payment_attempt",
                "payment attempt does not match its signed intent",
            ));
        }
        let offer_event = Event::from_json(&attempt.offer_event_json)
            .map_err(|error| WalletError::new("invalid_payment_attempt", error.to_string()))?;
        let recipient = recipient_offer(&offer_event, &attempt.recipient_pubkey)?;
        if recipient.offer != attempt.offer {
            return Err(WalletError::new(
                "invalid_payment_attempt",
                "payment attempt does not match its signed offer",
            ));
        }
        Ok(())
    }

    /// Bind a legacy attempt to the relay selected by an explicit user retry.
    pub fn bind_relay_if_missing(
        &self,
        attempt: &mut ZapAttempt,
        relay_url: &str,
    ) -> Result<(), WalletError> {
        if attempt.relay_url.is_some() {
            return Ok(());
        }
        attempt.relay_url = Some(relay_url.to_string());
        self.save(attempt)
    }

    fn save(&self, attempt: &mut ZapAttempt) -> Result<(), WalletError> {
        attempt.touch();
        super::ensure_private_directory(&self.directory)
            .map_err(|error| WalletError::unavailable(format!("create zap store: {error}")))?;
        let path = self.path(&attempt.idempotency_key)?;
        let bytes = serde_json::to_vec_pretty(attempt)
            .map_err(|error| WalletError::unavailable(format!("encode zap attempt: {error}")))?;
        let mut file = super::private_atomic_file(path)
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
        self.validate_loaded_attempt(&attempt.idempotency_key, attempt)?;
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
        let action = match payment.status {
            WalletPaymentStatus::Completed => {
                attempt.state = ZapAttemptState::PaidWithoutProof;
                WalletTraceAction::RecordPaidWithoutProof
            }
            WalletPaymentStatus::Failed => {
                attempt.state = ZapAttemptState::Failed;
                WalletTraceAction::RecordFailed {
                    payment_recorded: true,
                }
            }
            WalletPaymentStatus::Pending => WalletTraceAction::RecordPending,
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

    /// Persist the exact signed payer proof before relay publication.
    pub fn prepare_proof(
        &self,
        attempt: &mut ZapAttempt,
        keys: &Keys,
        channel_id: Option<&str>,
    ) -> Result<Event, WalletError> {
        if let Some(event) = attempt.proof_event()? {
            return Ok(event);
        }
        let event = attempt.build_proof_event(keys, channel_id)?;
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
        relay_url: &str,
    ) -> Result<Option<WalletProfileZapDraft>, WalletError> {
        let mut latest: Option<ZapAttempt> = None;
        for attempt in self.stored_attempts()? {
            let needs_reconciliation = matches!(
                attempt.state,
                ZapAttemptState::Prepared | ZapAttemptState::Paying
            ) || (attempt.state == ZapAttemptState::PaidWithoutProof
                && !attempt.proof_published);
            if attempt.recipient_pubkey == recipient_pubkey
                && attempt.target_event_id.as_deref() == target_event_id
                && attempt.relay_url.as_deref().is_none_or(|relay| {
                    relay.trim_end_matches('/') == relay_url.trim_end_matches('/')
                })
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

    /// List settled proofs that can be retried safely on the specified relay.
    pub fn unpublished_proofs_for_relay(
        &self,
        relay_url: &str,
    ) -> Result<Vec<ZapAttempt>, WalletError> {
        let relay_url = relay_url.trim_end_matches('/');
        let mut attempts = self
            .stored_attempts()?
            .into_iter()
            .filter(|attempt| {
                attempt.state == ZapAttemptState::PaidWithoutProof
                    && !attempt.proof_published
                    && attempt
                        .payment
                        .as_ref()
                        .is_some_and(|payment| payment.status == WalletPaymentStatus::Completed)
                    && attempt
                        .relay_url
                        .as_deref()
                        .is_some_and(|relay| relay.trim_end_matches('/') == relay_url)
            })
            .collect::<Vec<_>>();
        attempts.sort_by_key(|attempt| attempt.updated_at_ms);
        Ok(attempts)
    }

    /// List dispatched payments that still need a terminal provider result.
    ///
    /// A provider call can return or be recovered while the payment is still
    /// pending. These checkpoints must be revisited in the background: the
    /// recipient cannot discover a settled zap until the payer publishes its
    /// kind-9736 proof.
    pub fn paying_attempts_for_relay(
        &self,
        relay_url: &str,
    ) -> Result<Vec<ZapAttempt>, WalletError> {
        let relay_url = relay_url.trim_end_matches('/');
        let mut attempts = self
            .stored_attempts()?
            .into_iter()
            .filter(|attempt| {
                attempt.state == ZapAttemptState::Paying
                    && attempt
                        .relay_url
                        .as_deref()
                        .is_some_and(|relay| relay.trim_end_matches('/') == relay_url)
            })
            .collect::<Vec<_>>();
        attempts.sort_by_key(|attempt| attempt.updated_at_ms);
        Ok(attempts)
    }

    /// Remove terminal checkpoints after the documented 90-day retention
    /// period. Incomplete attempts are retained until reconciled.
    pub fn prune(&self) -> Result<(), WalletError> {
        let cutoff = now_ms().saturating_sub(TERMINAL_ATTEMPT_RETENTION_MS);
        for attempt in self.stored_attempts()? {
            if attempt.state.is_terminal() && attempt.updated_at_ms < cutoff {
                let path = self.path(&attempt.idempotency_key)?;
                if let Err(error) = std::fs::remove_file(&path) {
                    tracing::warn!(path = %path.display(), error = %error, "prune wallet attempt");
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
#[path = "zap/tests.rs"]
mod tests;

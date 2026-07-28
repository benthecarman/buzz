use std::{
    io::Write,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use atomic_write_file::AtomicWriteFile;
use buzz_core_pkg::kind::{KIND_BOLT12_OFFER, KIND_BOLT12_ZAP_INTENT};
use nostr::{Event, EventBuilder, JsonUtil, Keys, Kind, Tag};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::lexe_provider::canonical_offer;
use super::models::{
    WalletError, WalletPaymentResult, WalletProfileZapDraft, WalletProfileZapResult,
    WalletRecipientOffer,
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

pub fn recipient_offer(
    event: &Event,
    recipient_pubkey: &str,
) -> Result<WalletRecipientOffer, WalletError> {
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
    let offer = event
        .tags
        .iter()
        .filter_map(|tag| {
            let parts = tag.as_slice();
            (parts.first().map(String::as_str) == Some("offer"))
                .then(|| parts.get(1).cloned())
                .flatten()
        })
        .find(|offer| validate_canonical_offer(offer).is_ok())
        .ok_or_else(|| {
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

pub fn signed_intent(
    keys: &Keys,
    recipient: &WalletRecipientOffer,
    amount: u64,
    comment: &str,
) -> Result<Event, WalletError> {
    let tags = vec![
        tag(["p", recipient.recipient_pubkey.as_str()])?,
        tag(["amount", amount_msats(amount)?.as_str()])?,
        tag(["offer_event", recipient.offer_event_json.as_str()])?,
        tag(["zap_id", random_zap_id()?.as_str()])?,
    ];
    EventBuilder::new(Kind::Custom(KIND_BOLT12_ZAP_INTENT as u16), comment)
        .tags(tags)
        .sign_with_keys(keys)
        .map_err(|error| WalletError::new("invalid_zap", format!("sign zap intent: {error}")))
}

/// Durable checkpoints for a profile payment.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ZapAttemptState {
    /// Intent and recipient offer are persisted; no payment has started.
    Prepared,
    /// Payment may have reached Lexe and must be reconciled before retrying.
    Paying,
    /// Payment settled, but no public event can be produced without an `lnp`
    /// payer proof.
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
    /// Whole base-unit bitcoins paid to the recipient.
    pub amount: u64,
    pub comment: Option<String>,
    /// Canonical BOLT12 offer frozen before payment.
    pub offer: String,
    /// Exact recipient-signed offer event embedded by the intent.
    pub offer_event_json: String,
    /// Exact payer-signed, unbroadcast kind `9737` intent.
    pub intent_event_json: String,
    pub intent_event_id: String,
    /// Intent reference sent in the BOLT12 payer note for reconciliation.
    pub payer_note: String,
    pub state: ZapAttemptState,
    pub payment: Option<WalletPaymentResult>,
    #[serde(default)]
    pub updated_at_ms: u64,
}

impl ZapAttempt {
    pub fn prepare(
        idempotency_key: String,
        recipient: WalletRecipientOffer,
        amount: u64,
        comment: Option<String>,
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
        )?;
        let intent_event_id = intent.id.to_hex();
        Ok(Self {
            version: 2,
            idempotency_key,
            recipient_pubkey: recipient.recipient_pubkey,
            amount,
            comment,
            offer: recipient.offer,
            offer_event_json: recipient.offer_event_json,
            intent_event_json: intent.as_json(),
            payer_note: format!("nostr:nipB1:{intent_event_id}"),
            intent_event_id,
            state: ZapAttemptState::Prepared,
            payment: None,
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
            proof_published: false,
        })
    }

    fn draft(&self) -> WalletProfileZapDraft {
        WalletProfileZapDraft {
            recipient_pubkey: self.recipient_pubkey.clone(),
            amount: self.amount,
            comment: self.comment.clone(),
            idempotency_key: self.idempotency_key.clone(),
        }
    }
}

/// Atomic, identity-scoped persistence for profile-payment checkpoints.
pub struct ZapAttemptStore {
    directory: PathBuf,
}

impl ZapAttemptStore {
    pub fn new(app_data_dir: &Path, payer_pubkey: &str) -> Self {
        Self {
            directory: app_data_dir
                .join("wallet")
                .join("zap-attempts")
                .join(payer_pubkey),
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

    pub fn save(&self, attempt: &mut ZapAttempt) -> Result<(), WalletError> {
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

    pub fn pending_for_recipient(
        &self,
        recipient_pubkey: &str,
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
            if attempt.recipient_pubkey == recipient_pubkey
                && matches!(
                    attempt.state,
                    ZapAttemptState::Prepared | ZapAttemptState::Paying
                )
                && latest
                    .as_ref()
                    .is_none_or(|current| attempt.updated_at_ms > current.updated_at_ms)
            {
                latest = Some(attempt);
            }
        }
        Ok(latest.map(|attempt| attempt.draft()))
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

    #[test]
    fn intent_matches_protocol_shape_and_uses_nip_b1_note() {
        let payer = Keys::generate();
        let recipient_keys = Keys::generate();
        let attempt = ZapAttempt::prepare(
            Uuid::new_v4().to_string(),
            recipient(&recipient_keys),
            21,
            Some("great work".to_string()),
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
    fn parses_only_canonical_lowercase_offers() {
        assert!(build_offer_announcement(VALID_OFFER).is_ok());
        assert!(build_offer_announcement(&VALID_OFFER.to_ascii_uppercase()).is_err());
        assert!(build_offer_announcement(&VALID_OFFER[..VALID_OFFER.len() - 1]).is_err());
        assert!(build_offer_announcement(&format!("{VALID_OFFER} ")).is_err());
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
            &payer,
        )
        .unwrap();
        let store = ZapAttemptStore::new(temp.path(), &payer.public_key().to_hex());
        store.save(&mut attempt).unwrap();
        assert_eq!(
            store.load(&attempt.idempotency_key).unwrap(),
            Some(attempt.clone())
        );
        assert_eq!(
            store
                .pending_for_recipient(&attempt.recipient_pubkey)
                .unwrap()
                .unwrap()
                .idempotency_key,
            attempt.idempotency_key
        );
    }
}

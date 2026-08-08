//! Buzz Agent Runtime Payments wire types and deterministic ledger reducer.
//!
//! Pricing is advertised independently from BOLT12 offer kind `10058`.
//! Deposits add non-expiring runtime milliseconds, reservations lock a cap,
//! and settlements replace that lock with measured usage.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Current wire-format version.
pub const VERSION: u8 = 1;
/// Supported prepaid runtime pack sizes.
pub const RUNTIME_PACKS_MINUTES: [u16; 3] = [15, 30, 60];
/// Largest rate whose 60-minute charge remains an exact JavaScript integer.
pub const MAX_RUNTIME_RATE_SATS_PER_MINUTE: u64 = 150_119_987_579_016;
/// Milliseconds in one runtime minute.
pub const MILLIS_PER_MINUTE: u64 = 60_000;
/// Maximum byte length for caller-provided idempotency identifiers.
pub const MAX_REQUEST_ID_BYTES: usize = 128;

/// Decrypted content of an ephemeral kind `24210` reservation request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuntimeReservationRequest {
    /// Schema version.
    pub version: u8,
    /// Payer-generated idempotency identifier.
    pub request_id: String,
    /// Non-DM channel in which the instruction will be published.
    pub channel_id: String,
    /// Selected invocation cap. Must be 15, 30, or 60.
    pub cap_minutes: u16,
}

impl RuntimeReservationRequest {
    /// Validate the request before authorization checks are applied.
    pub fn validate(&self) -> Result<(), RuntimePaymentError> {
        validate_version(self.version)?;
        validate_request_id(&self.request_id)?;
        if self.channel_id.is_empty() || self.channel_id.len() > MAX_REQUEST_ID_BYTES {
            return Err(RuntimePaymentError::InvalidRequest(
                "channel_id must contain 1 to 128 bytes",
            ));
        }
        validate_pack(self.cap_minutes)
    }
}

/// Agent-authored price-locked quote returned in kind `24211`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuntimeQuote {
    /// Schema version.
    pub version: u8,
    /// Request identifier from the reservation request.
    pub request_id: String,
    /// Agent receiving the payment and providing runtime.
    pub agent_pubkey: String,
    /// User paying for the runtime pack.
    pub payer_pubkey: String,
    /// Purchase and invocation community channel.
    pub channel_id: String,
    /// Selected invocation cap.
    pub cap_minutes: u16,
    /// Purchased pack size. Must equal `cap_minutes`.
    pub pack_minutes: u16,
    /// Locked whole-satoshi rate.
    pub price_per_minute_sats: u64,
    /// Exact required amount in satoshis.
    pub amount_sats: u64,
    /// Exact signed kind-10058 offer event. Pricing is never placed in it.
    pub offer_event: serde_json::Value,
    /// Quote expiration as a Unix timestamp.
    pub expires_at: u64,
}

impl RuntimeQuote {
    /// Validate immutable quote terms and checked price arithmetic.
    pub fn validate(&self) -> Result<(), RuntimePaymentError> {
        validate_version(self.version)?;
        validate_request_id(&self.request_id)?;
        validate_hex_pubkey(&self.agent_pubkey)?;
        validate_hex_pubkey(&self.payer_pubkey)?;
        if self.agent_pubkey == self.payer_pubkey {
            return Err(RuntimePaymentError::InvalidQuote(
                "payer and agent must be different pubkeys",
            ));
        }
        if self.channel_id.is_empty() || self.channel_id.len() > MAX_REQUEST_ID_BYTES {
            return Err(RuntimePaymentError::InvalidQuote(
                "channel_id must contain 1 to 128 bytes",
            ));
        }
        validate_pack(self.cap_minutes)?;
        validate_pack(self.pack_minutes)?;
        if self.pack_minutes != self.cap_minutes {
            return Err(RuntimePaymentError::InvalidQuote(
                "pack_minutes must equal cap_minutes",
            ));
        }
        if self.price_per_minute_sats == 0
            || self.price_per_minute_sats > MAX_RUNTIME_RATE_SATS_PER_MINUTE
        {
            return Err(RuntimePaymentError::InvalidQuote(
                "price_per_minute_sats is outside the supported whole-satoshi range",
            ));
        }
        let expected_amount = self
            .price_per_minute_sats
            .checked_mul(u64::from(self.pack_minutes))
            .ok_or(RuntimePaymentError::ArithmeticOverflow)?;
        if self.amount_sats != expected_amount {
            return Err(RuntimePaymentError::InvalidQuote(
                "amount_sats does not match price and pack",
            ));
        }
        if !self.offer_event.is_object() {
            return Err(RuntimePaymentError::InvalidQuote(
                "offer_event must be an exact signed event object",
            ));
        }
        if self.expires_at == 0 {
            return Err(RuntimePaymentError::InvalidQuote(
                "expires_at must be a positive unix timestamp",
            ));
        }
        Ok(())
    }
}

/// Decrypted content of an ephemeral kind `24211` response.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum RuntimeReservationResponse {
    /// Runtime was atomically reserved from retained balance.
    Reserved {
        /// Schema version.
        version: u8,
        /// Correlated payer request.
        request_id: String,
        /// Exact signed kind-44211 reservation event.
        reservation_event: serde_json::Value,
    },
    /// A BOLT12 payment is required before reservation can succeed.
    PaymentRequired {
        /// Flattened immutable quote terms.
        #[serde(flatten)]
        quote: RuntimeQuote,
    },
    /// Generic failure that intentionally reveals no authorization detail.
    Unavailable {
        /// Schema version.
        version: u8,
        /// Correlated payer request.
        request_id: String,
    },
}

impl RuntimeReservationResponse {
    /// Validate response correlation fields and embedded signed-event shapes.
    pub fn validate(&self) -> Result<(), RuntimePaymentError> {
        match self {
            Self::Reserved {
                version,
                request_id,
                reservation_event,
            } => {
                validate_version(*version)?;
                validate_request_id(request_id)?;
                if !reservation_event.is_object() {
                    return Err(RuntimePaymentError::InvalidResponse(
                        "reservation_event must be an exact signed event object",
                    ));
                }
                Ok(())
            }
            Self::PaymentRequired { quote } => quote.validate(),
            Self::Unavailable {
                version,
                request_id,
            } => {
                validate_version(*version)?;
                validate_request_id(request_id)
            }
        }
    }
}

/// Public, agent-authored pricing terms carried by replaceable kind `10101`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuntimePricing {
    /// Schema version. Must equal [`VERSION`].
    pub version: u8,
    /// Whether paid runtime is currently offered.
    pub enabled: bool,
    /// Whole satoshis charged per runtime minute. Required only when enabled.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rate_sats_per_minute: Option<u64>,
    /// Supported pack sizes. Required to equal [`RUNTIME_PACKS_MINUTES`] when enabled.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub runtime_packs_minutes: Vec<u16>,
}

impl RuntimePricing {
    /// Construct enabled pricing with Buzz's fixed packs.
    pub fn enabled(rate_sats_per_minute: u64) -> Result<Self, RuntimePaymentError> {
        let pricing = Self {
            version: VERSION,
            enabled: true,
            rate_sats_per_minute: Some(rate_sats_per_minute),
            runtime_packs_minutes: RUNTIME_PACKS_MINUTES.to_vec(),
        };
        pricing.validate()?;
        Ok(pricing)
    }

    /// Construct an explicit disabled tombstone that supersedes stale pricing.
    pub fn disabled() -> Self {
        Self {
            version: VERSION,
            enabled: false,
            rate_sats_per_minute: None,
            runtime_packs_minutes: Vec::new(),
        }
    }

    /// Validate canonical pricing terms.
    pub fn validate(&self) -> Result<(), RuntimePaymentError> {
        validate_version(self.version)?;
        if self.enabled {
            let rate = self
                .rate_sats_per_minute
                .ok_or(RuntimePaymentError::InvalidPricing(
                    "enabled pricing requires a rate",
                ))?;
            if rate == 0 || rate > MAX_RUNTIME_RATE_SATS_PER_MINUTE {
                return Err(RuntimePaymentError::InvalidPricing(
                    "runtime price is outside the supported whole-satoshi range",
                ));
            }
            if self.runtime_packs_minutes != RUNTIME_PACKS_MINUTES {
                return Err(RuntimePaymentError::InvalidPricing(
                    "runtime packs must be exactly 15, 30, and 60 minutes",
                ));
            }
        } else if self.rate_sats_per_minute.is_some() || !self.runtime_packs_minutes.is_empty() {
            return Err(RuntimePaymentError::InvalidPricing(
                "disabled pricing must not carry a rate or packs",
            ));
        }
        Ok(())
    }
}

/// Plaintext content of a settled kind `44210` runtime-credit deposit.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuntimeDeposit {
    /// Schema version.
    pub version: u8,
    /// Purchased pack duration.
    pub pack_minutes: u16,
    /// Runtime milliseconds credited by the pack.
    pub credit_ms: u64,
    /// Locked whole-satoshi rate.
    pub price_per_minute_sats: u64,
    /// Exact zap amount in satoshis.
    pub amount_sats: u64,
}

impl RuntimeDeposit {
    /// Validate pack, credit, and price arithmetic.
    pub fn validate(&self) -> Result<(), RuntimePaymentError> {
        validate_version(self.version)?;
        validate_pack(self.pack_minutes)?;
        let expected_credit = u64::from(self.pack_minutes)
            .checked_mul(MILLIS_PER_MINUTE)
            .ok_or(RuntimePaymentError::ArithmeticOverflow)?;
        if self.credit_ms != expected_credit {
            return Err(RuntimePaymentError::InvalidDeposit(
                "credit_ms does not match pack_minutes",
            ));
        }
        if self.price_per_minute_sats == 0 {
            return Err(RuntimePaymentError::InvalidDeposit(
                "price_per_minute_sats must be greater than zero",
            ));
        }
        let expected_amount = self
            .price_per_minute_sats
            .checked_mul(u64::from(self.pack_minutes))
            .ok_or(RuntimePaymentError::ArithmeticOverflow)?;
        if self.amount_sats != expected_amount {
            return Err(RuntimePaymentError::InvalidDeposit(
                "amount_sats does not match price and pack",
            ));
        }
        Ok(())
    }
}

/// Decrypted content of an agent-signed kind `44211` reservation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuntimeReservation {
    /// Schema version.
    pub version: u8,
    /// Payer-generated idempotency identifier.
    pub request_id: String,
    /// Maximum billable runtime for this invocation.
    pub cap_ms: u64,
    /// Unix timestamp by which the reservation must begin.
    pub must_start_by: u64,
}

impl RuntimeReservation {
    /// Validate the reservation payload.
    pub fn validate(&self) -> Result<(), RuntimePaymentError> {
        validate_version(self.version)?;
        if self.request_id.is_empty() || self.request_id.len() > 128 {
            return Err(RuntimePaymentError::InvalidReservation(
                "request_id must contain 1 to 128 bytes",
            ));
        }
        let minutes = self.cap_ms / MILLIS_PER_MINUTE;
        if !self.cap_ms.is_multiple_of(MILLIS_PER_MINUTE)
            || !RUNTIME_PACKS_MINUTES
                .iter()
                .any(|pack| u64::from(*pack) == minutes)
        {
            return Err(RuntimePaymentError::InvalidReservation(
                "cap_ms must equal a supported runtime pack",
            ));
        }
        if self.must_start_by == 0 {
            return Err(RuntimePaymentError::InvalidReservation(
                "must_start_by must be a positive unix timestamp",
            ));
        }
        Ok(())
    }
}

/// Terminal outcome of a metered runtime reservation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeOutcome {
    /// Agent completed successfully.
    Completed,
    /// Agent or provider returned an error.
    Error,
    /// Payer cancelled the turn.
    Cancelled,
    /// Idle or hard timeout fired.
    Timeout,
    /// The reserved runtime cap was fully consumed.
    BudgetExhausted,
    /// The harness terminated unexpectedly.
    Interrupted,
    /// Reservation expired before execution began.
    UnusedExpired,
}

/// Decrypted content of an agent-signed kind `44212` settlement.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuntimeSettlement {
    /// Schema version.
    pub version: u8,
    /// Referenced reservation event id.
    pub reservation_id: String,
    /// Instruction event that consumed the reservation, absent when unused.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub instruction_event_id: Option<String>,
    /// Reserved maximum runtime.
    pub cap_ms: u64,
    /// Measured billable runtime.
    pub used_ms: u64,
    /// Terminal outcome.
    pub outcome: RuntimeOutcome,
}

impl RuntimeSettlement {
    /// Validate usage bounds and outcome-specific requirements.
    pub fn validate(&self) -> Result<(), RuntimePaymentError> {
        validate_version(self.version)?;
        if self.reservation_id.is_empty() || self.reservation_id.len() > 128 {
            return Err(RuntimePaymentError::InvalidSettlement(
                "reservation_id must contain 1 to 128 bytes",
            ));
        }
        if self.used_ms > self.cap_ms {
            return Err(RuntimePaymentError::InvalidSettlement(
                "used_ms exceeds cap_ms",
            ));
        }
        if self.outcome == RuntimeOutcome::BudgetExhausted && self.used_ms != self.cap_ms {
            return Err(RuntimePaymentError::InvalidSettlement(
                "budget_exhausted must consume the full cap",
            ));
        }
        if self.outcome == RuntimeOutcome::UnusedExpired
            && (self.used_ms != 0 || self.instruction_event_id.is_some())
        {
            return Err(RuntimePaymentError::InvalidSettlement(
                "unused_expired must have zero usage and no instruction",
            ));
        }
        Ok(())
    }
}

/// Deposit input for deterministic ledger replay.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DepositRecord {
    /// Zap intent identifier; unique per credited payment.
    pub payment_id: String,
    /// Credited runtime milliseconds.
    pub credit_ms: u64,
}

/// Reservation input for deterministic ledger replay.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReservationRecord {
    /// Reservation event identifier.
    pub reservation_id: String,
    /// Locked runtime cap.
    pub cap_ms: u64,
}

/// Settlement input for deterministic ledger replay.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SettlementRecord {
    /// Referenced reservation identifier.
    pub reservation_id: String,
    /// Final billable usage.
    pub used_ms: u64,
}

/// Deterministic projection of an append-only runtime ledger.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RuntimeLedger {
    credited_ms: u64,
    used_ms: u64,
    payments: BTreeMap<String, u64>,
    reservations: BTreeMap<String, u64>,
    settlements: BTreeMap<String, u64>,
}

impl RuntimeLedger {
    /// Apply a settled credit deposit exactly once.
    pub fn apply_deposit(&mut self, record: DepositRecord) -> Result<(), RuntimePaymentError> {
        if record.payment_id.is_empty() || record.credit_ms == 0 {
            return Err(RuntimePaymentError::InvalidLedgerEntry);
        }
        if let Some(existing) = self.payments.get(&record.payment_id) {
            return if *existing == record.credit_ms {
                Ok(())
            } else {
                Err(RuntimePaymentError::ConflictingDuplicate)
            };
        }
        self.credited_ms = self
            .credited_ms
            .checked_add(record.credit_ms)
            .ok_or(RuntimePaymentError::ArithmeticOverflow)?;
        self.payments.insert(record.payment_id, record.credit_ms);
        Ok(())
    }

    /// Lock runtime for a pending invocation.
    pub fn apply_reservation(
        &mut self,
        record: ReservationRecord,
    ) -> Result<(), RuntimePaymentError> {
        if record.reservation_id.is_empty() || record.cap_ms == 0 {
            return Err(RuntimePaymentError::InvalidLedgerEntry);
        }
        if let Some(existing) = self.reservations.get(&record.reservation_id) {
            return if *existing == record.cap_ms {
                Ok(())
            } else {
                Err(RuntimePaymentError::ConflictingDuplicate)
            };
        }
        if self.available_ms()? < record.cap_ms {
            return Err(RuntimePaymentError::InsufficientRuntime);
        }
        self.reservations
            .insert(record.reservation_id, record.cap_ms);
        Ok(())
    }

    /// Close a reservation and replace its cap lock with measured usage.
    pub fn apply_settlement(
        &mut self,
        record: SettlementRecord,
    ) -> Result<(), RuntimePaymentError> {
        if let Some(existing) = self.settlements.get(&record.reservation_id) {
            return if *existing == record.used_ms {
                Ok(())
            } else {
                Err(RuntimePaymentError::ConflictingDuplicate)
            };
        }
        let cap_ms = self
            .reservations
            .get(&record.reservation_id)
            .copied()
            .ok_or(RuntimePaymentError::UnknownReservation)?;
        if record.used_ms > cap_ms {
            return Err(RuntimePaymentError::UsageExceedsReservation);
        }
        self.used_ms = self
            .used_ms
            .checked_add(record.used_ms)
            .ok_or(RuntimePaymentError::ArithmeticOverflow)?;
        self.settlements
            .insert(record.reservation_id, record.used_ms);
        Ok(())
    }

    /// Runtime milliseconds not used or locked by an open reservation.
    pub fn available_ms(&self) -> Result<u64, RuntimePaymentError> {
        let locked = self
            .reservations
            .iter()
            .filter(|(id, _)| !self.settlements.contains_key(*id))
            .map(|(_, cap)| *cap)
            .try_fold(0u64, |sum, value| sum.checked_add(value))
            .ok_or(RuntimePaymentError::ArithmeticOverflow)?;
        self.credited_ms
            .checked_sub(self.used_ms)
            .and_then(|value| value.checked_sub(locked))
            .ok_or(RuntimePaymentError::LedgerUnderflow)
    }

    /// Total credited runtime.
    pub fn credited_ms(&self) -> u64 {
        self.credited_ms
    }

    /// Total settled billable runtime.
    pub fn used_ms(&self) -> u64 {
        self.used_ms
    }

    /// Whether a known reservation still holds runtime and has no settlement.
    pub fn reservation_is_open(&self, reservation_id: &str) -> bool {
        self.reservations.contains_key(reservation_id)
            && !self.settlements.contains_key(reservation_id)
    }

    /// The cap locked by a known reservation.
    pub fn reservation_cap_ms(&self, reservation_id: &str) -> Option<u64> {
        self.reservations.get(reservation_id).copied()
    }
}

/// Validation or ledger error.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum RuntimePaymentError {
    /// Unknown schema version.
    #[error("unsupported runtime-payment version")]
    UnsupportedVersion,
    /// Invalid public pricing terms.
    #[error("invalid runtime pricing: {0}")]
    InvalidPricing(&'static str),
    /// Invalid reservation request payload.
    #[error("invalid runtime reservation request: {0}")]
    InvalidRequest(&'static str),
    /// Invalid signed quote terms.
    #[error("invalid runtime quote: {0}")]
    InvalidQuote(&'static str),
    /// Invalid reservation response payload.
    #[error("invalid runtime reservation response: {0}")]
    InvalidResponse(&'static str),
    /// Unsupported pack size.
    #[error("runtime pack must be 15, 30, or 60 minutes")]
    InvalidPack,
    /// Invalid deposit payload.
    #[error("invalid runtime deposit: {0}")]
    InvalidDeposit(&'static str),
    /// Invalid reservation payload.
    #[error("invalid runtime reservation: {0}")]
    InvalidReservation(&'static str),
    /// Invalid settlement payload.
    #[error("invalid runtime settlement: {0}")]
    InvalidSettlement(&'static str),
    /// Integer arithmetic overflowed.
    #[error("runtime-payment arithmetic overflow")]
    ArithmeticOverflow,
    /// Ledger entry lacks a required id or amount.
    #[error("invalid runtime ledger entry")]
    InvalidLedgerEntry,
    /// A duplicate identifier carried different immutable data.
    #[error("conflicting duplicate runtime ledger entry")]
    ConflictingDuplicate,
    /// Available credit cannot cover a requested cap.
    #[error("insufficient runtime credit")]
    InsufficientRuntime,
    /// Settlement references no open reservation.
    #[error("settlement references an unknown reservation")]
    UnknownReservation,
    /// Settlement usage exceeds its reservation.
    #[error("settlement usage exceeds reservation cap")]
    UsageExceedsReservation,
    /// Ledger totals would become negative.
    #[error("runtime ledger underflow")]
    LedgerUnderflow,
}

fn validate_version(version: u8) -> Result<(), RuntimePaymentError> {
    if version == VERSION {
        Ok(())
    } else {
        Err(RuntimePaymentError::UnsupportedVersion)
    }
}

fn validate_pack(minutes: u16) -> Result<(), RuntimePaymentError> {
    if RUNTIME_PACKS_MINUTES.contains(&minutes) {
        Ok(())
    } else {
        Err(RuntimePaymentError::InvalidPack)
    }
}

fn validate_request_id(request_id: &str) -> Result<(), RuntimePaymentError> {
    if request_id.is_empty() || request_id.len() > MAX_REQUEST_ID_BYTES {
        Err(RuntimePaymentError::InvalidRequest(
            "request_id must contain 1 to 128 bytes",
        ))
    } else {
        Ok(())
    }
}

fn validate_hex_pubkey(pubkey: &str) -> Result<(), RuntimePaymentError> {
    if pubkey.len() == 64 && pubkey.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        Ok(())
    } else {
        Err(RuntimePaymentError::InvalidQuote(
            "agent and payer pubkeys must be 64 hexadecimal characters",
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pricing_is_canonical_and_independent_of_offer_data() {
        let pricing = RuntimePricing::enabled(20).unwrap();
        assert_eq!(pricing.runtime_packs_minutes, [15, 30, 60]);
        assert!(!serde_json::to_string(&pricing).unwrap().contains("offer"));
        assert!(RuntimePricing::enabled(0).is_err());
        assert!(RuntimePricing::enabled(MAX_RUNTIME_RATE_SATS_PER_MINUTE).is_ok());
        assert!(RuntimePricing::enabled(MAX_RUNTIME_RATE_SATS_PER_MINUTE + 1).is_err());
    }

    #[test]
    fn deposit_arithmetic_is_exact() {
        RuntimeDeposit {
            version: VERSION,
            pack_minutes: 30,
            credit_ms: 1_800_000,
            price_per_minute_sats: 20,
            amount_sats: 600,
        }
        .validate()
        .unwrap();
    }

    #[test]
    fn ledger_locks_cap_and_returns_unused_runtime() {
        let mut ledger = RuntimeLedger::default();
        ledger
            .apply_deposit(DepositRecord {
                payment_id: "payment-1".into(),
                credit_ms: 1_800_000,
            })
            .unwrap();
        ledger
            .apply_reservation(ReservationRecord {
                reservation_id: "reservation-1".into(),
                cap_ms: 900_000,
            })
            .unwrap();
        assert_eq!(ledger.available_ms().unwrap(), 900_000);
        ledger
            .apply_settlement(SettlementRecord {
                reservation_id: "reservation-1".into(),
                used_ms: 41_237,
            })
            .unwrap();
        assert_eq!(ledger.available_ms().unwrap(), 1_758_763);
    }

    #[test]
    fn duplicate_payment_is_idempotent() {
        let mut ledger = RuntimeLedger::default();
        for _ in 0..2 {
            ledger
                .apply_deposit(DepositRecord {
                    payment_id: "same".into(),
                    credit_ms: 900_000,
                })
                .unwrap();
        }
        assert_eq!(ledger.credited_ms(), 900_000);
    }

    #[test]
    fn concurrent_reservations_cannot_overspend() {
        let mut ledger = RuntimeLedger::default();
        ledger
            .apply_deposit(DepositRecord {
                payment_id: "payment".into(),
                credit_ms: 900_000,
            })
            .unwrap();
        ledger
            .apply_reservation(ReservationRecord {
                reservation_id: "first".into(),
                cap_ms: 900_000,
            })
            .unwrap();
        assert_eq!(
            ledger.apply_reservation(ReservationRecord {
                reservation_id: "second".into(),
                cap_ms: 900_000,
            }),
            Err(RuntimePaymentError::InsufficientRuntime)
        );
    }

    #[test]
    fn budget_exhausted_requires_full_cap() {
        assert!(RuntimeSettlement {
            version: VERSION,
            reservation_id: "reservation".into(),
            instruction_event_id: Some("instruction".into()),
            cap_ms: 900_000,
            used_ms: 899_999,
            outcome: RuntimeOutcome::BudgetExhausted,
        }
        .validate()
        .is_err());
    }
}

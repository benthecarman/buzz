use std::{
    io::Write,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use atomic_write_file::AtomicWriteFile;
use buzz_conformance_pkg::wallet::{WalletAbstractState, WalletAttemptStatus, WalletTraceAction};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::{
    conformance,
    models::{WalletError, WalletPaymentResult, WalletSendRequest},
};

const TERMINAL_ATTEMPT_RETENTION_MS: u64 = 90 * 24 * 60 * 60 * 1_000;

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis().min(u128::from(u64::MAX)) as u64)
        .unwrap_or_default()
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SendAttemptState {
    Prepared,
    Paying,
    Completed,
    Failed,
}

impl SendAttemptState {
    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Completed | Self::Failed)
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct SendAttempt {
    pub version: u8,
    pub request: WalletSendRequest,
    pub state: SendAttemptState,
    pub payment: Option<WalletPaymentResult>,
    pub updated_at_ms: u64,
}

impl SendAttempt {
    pub fn prepare(request: WalletSendRequest) -> Self {
        Self {
            version: 1,
            request,
            state: SendAttemptState::Prepared,
            payment: None,
            updated_at_ms: now_ms(),
        }
    }

    pub fn touch(&mut self) {
        self.updated_at_ms = now_ms();
    }

    fn abstract_state(&self) -> WalletAbstractState {
        let status = match self.state {
            SendAttemptState::Prepared => WalletAttemptStatus::GenericPrepared,
            SendAttemptState::Paying => WalletAttemptStatus::GenericPaying,
            SendAttemptState::Completed => WalletAttemptStatus::GenericCompleted,
            SendAttemptState::Failed => WalletAttemptStatus::GenericFailed,
        };
        WalletAbstractState {
            status,
            payment_recorded: self.payment.is_some(),
        }
    }
}

pub struct SendAttemptStore {
    directory: PathBuf,
    payer_pubkey: String,
}

impl SendAttemptStore {
    pub fn new(app_data_dir: &Path, payer_pubkey: &str) -> Self {
        Self {
            directory: app_data_dir
                .join("wallet")
                .join("send-attempts")
                .join(payer_pubkey),
            payer_pubkey: payer_pubkey.to_string(),
        }
    }

    fn path(&self, request_id: &str) -> Result<PathBuf, WalletError> {
        let id = Uuid::parse_str(request_id).map_err(|_| {
            WalletError::new(
                "invalid_idempotency_key",
                "wallet payment request ID must be a UUID",
            )
        })?;
        Ok(self.directory.join(format!("{id}.json")))
    }

    pub fn load(&self, request_id: &str) -> Result<Option<SendAttempt>, WalletError> {
        let path = self.path(request_id)?;
        match std::fs::read(path) {
            Ok(bytes) => serde_json::from_slice(&bytes)
                .map(Some)
                .map_err(|error| WalletError::unavailable(format!("read send attempt: {error}"))),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(WalletError::unavailable(format!(
                "open send attempt: {error}"
            ))),
        }
    }

    fn save(&self, attempt: &mut SendAttempt) -> Result<(), WalletError> {
        attempt.touch();
        std::fs::create_dir_all(&self.directory)
            .map_err(|error| WalletError::unavailable(format!("create send store: {error}")))?;
        let bytes = serde_json::to_vec_pretty(attempt)
            .map_err(|error| WalletError::unavailable(format!("encode send attempt: {error}")))?;
        let mut file = AtomicWriteFile::open(self.path(&attempt.request.request_id)?)
            .map_err(|error| WalletError::unavailable(format!("open send attempt: {error}")))?;
        file.write_all(&bytes)
            .map_err(|error| WalletError::unavailable(format!("write send attempt: {error}")))?;
        file.commit()
            .map_err(|error| WalletError::unavailable(format!("commit send attempt: {error}")))
    }

    /// Persist a new attempt and emit the spec's `PrepareGeneric` action.
    pub fn save_prepared(&self, attempt: &mut SendAttempt) -> Result<(), WalletError> {
        if attempt.state != SendAttemptState::Prepared || attempt.payment.is_some() {
            return Err(WalletError::unavailable(
                "new send attempt is not in the prepared state",
            ));
        }
        self.save(attempt)?;
        conformance::record(
            &self.payer_pubkey,
            conformance::WalletAttemptKind::GenericSend,
            &attempt.request.request_id,
            WalletTraceAction::PrepareGeneric,
            WalletAbstractState::absent(),
            attempt.abstract_state(),
        );
        Ok(())
    }

    /// Durably enter `Paying` before the only provider send call.
    pub fn begin_dispatch(&self, attempt: &mut SendAttempt) -> Result<(), WalletError> {
        let before = attempt.abstract_state();
        if attempt.state != SendAttemptState::Prepared || attempt.payment.is_some() {
            return Err(WalletError::unavailable(
                "send attempt cannot dispatch from its current state",
            ));
        }
        attempt.state = SendAttemptState::Paying;
        self.save(attempt)?;
        conformance::record(
            &self.payer_pubkey,
            conformance::WalletAttemptKind::GenericSend,
            &attempt.request.request_id,
            WalletTraceAction::BeginDispatch,
            before,
            attempt.abstract_state(),
        );
        Ok(())
    }

    /// Record that this invocation chose provider reconciliation, not send.
    pub fn record_reconcile(&self, attempt: &SendAttempt) -> Result<(), WalletError> {
        if attempt.state != SendAttemptState::Paying {
            return Err(WalletError::unavailable(
                "send attempt cannot reconcile from its current state",
            ));
        }
        let state = attempt.abstract_state();
        conformance::record(
            &self.payer_pubkey,
            conformance::WalletAttemptKind::GenericSend,
            &attempt.request.request_id,
            WalletTraceAction::Reconcile,
            state,
            state,
        );
        Ok(())
    }

    /// Persist a provider result and emit the matching modeled transition.
    pub fn record_payment(
        &self,
        attempt: &mut SendAttempt,
        payment: WalletPaymentResult,
    ) -> Result<(), WalletError> {
        let before = attempt.abstract_state();
        if attempt.state != SendAttemptState::Paying {
            return Err(WalletError::unavailable(
                "send attempt cannot record payment from its current state",
            ));
        }
        let action = match payment.status.as_str() {
            "completed" => {
                attempt.state = SendAttemptState::Completed;
                WalletTraceAction::RecordCompleted
            }
            "failed" => {
                attempt.state = SendAttemptState::Failed;
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
            conformance::WalletAttemptKind::GenericSend,
            &attempt.request.request_id,
            action,
            before,
            attempt.abstract_state(),
        );
        Ok(())
    }

    /// Fail an expired reconciliation without inventing a provider result.
    pub fn fail_reconciliation(&self, attempt: &mut SendAttempt) -> Result<(), WalletError> {
        let before = attempt.abstract_state();
        if attempt.state != SendAttemptState::Paying {
            return Err(WalletError::unavailable(
                "send reconciliation cannot expire from its current state",
            ));
        }
        let payment_recorded = attempt.payment.is_some();
        attempt.state = SendAttemptState::Failed;
        self.save(attempt)?;
        conformance::record(
            &self.payer_pubkey,
            conformance::WalletAttemptKind::GenericSend,
            &attempt.request.request_id,
            WalletTraceAction::RecordFailed { payment_recorded },
            before,
            attempt.abstract_state(),
        );
        Ok(())
    }

    /// Emit a no-side-effect terminal replay decision.
    pub fn record_terminal_reuse(&self, attempt: &SendAttempt) {
        let state = attempt.abstract_state();
        conformance::record(
            &self.payer_pubkey,
            conformance::WalletAttemptKind::GenericSend,
            &attempt.request.request_id,
            WalletTraceAction::ReuseTerminal,
            state,
            state,
        );
    }

    /// Emit rejection of different details under an existing request ID.
    pub fn record_conflict(&self, attempt: &SendAttempt) {
        let state = attempt.abstract_state();
        conformance::record(
            &self.payer_pubkey,
            conformance::WalletAttemptKind::GenericSend,
            &attempt.request.request_id,
            WalletTraceAction::RejectConflict,
            state,
            state,
        );
    }

    pub fn latest_pending(&self) -> Result<Option<WalletSendRequest>, WalletError> {
        let entries = match std::fs::read_dir(&self.directory) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => {
                return Err(WalletError::unavailable(format!(
                    "read send attempt directory: {error}"
                )))
            }
        };
        let mut latest: Option<SendAttempt> = None;
        for entry in entries.flatten() {
            let Ok(bytes) = std::fs::read(entry.path()) else {
                continue;
            };
            let Ok(attempt) = serde_json::from_slice::<SendAttempt>(&bytes) else {
                continue;
            };
            if matches!(
                attempt.state,
                SendAttemptState::Prepared | SendAttemptState::Paying
            ) && latest
                .as_ref()
                .is_none_or(|current| attempt.updated_at_ms > current.updated_at_ms)
            {
                latest = Some(attempt);
            }
        }
        Ok(latest.map(|attempt| attempt.request))
    }

    pub fn prune(&self) -> Result<(), WalletError> {
        let entries = match std::fs::read_dir(&self.directory) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(error) => {
                return Err(WalletError::unavailable(format!(
                    "read send attempt directory: {error}"
                )))
            }
        };
        let cutoff = now_ms().saturating_sub(TERMINAL_ATTEMPT_RETENTION_MS);
        for entry in entries.flatten() {
            let path = entry.path();
            let Ok(bytes) = std::fs::read(&path) else {
                continue;
            };
            let Ok(attempt) = serde_json::from_slice::<SendAttempt>(&bytes) else {
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

    #[test]
    fn send_attempt_round_trips_and_restores_pending_request() {
        let temp = tempfile::tempdir().unwrap();
        let store = SendAttemptStore::new(temp.path(), &"a".repeat(64));
        let mut attempt = SendAttempt::prepare(WalletSendRequest {
            destination: VALID_OFFER.to_string(),
            amount: Some(21),
            message: Some("hello".to_string()),
            request_id: Uuid::new_v4().to_string(),
        });
        store.save_prepared(&mut attempt).unwrap();
        assert_eq!(
            store.load(&attempt.request.request_id).unwrap(),
            Some(attempt.clone())
        );
        assert_eq!(store.latest_pending().unwrap(), Some(attempt.request));
    }

    #[test]
    fn persisted_execution_emits_a_model_accepted_trace() {
        use buzz_conformance_pkg::wallet::{check_wallet_trace, WalletCheckerConfig};

        let _ = crate::wallet::conformance::take_test_trace();
        let temp = tempfile::tempdir().unwrap();
        let store = SendAttemptStore::new(temp.path(), &"b".repeat(64));
        let mut attempt = SendAttempt::prepare(WalletSendRequest {
            destination: VALID_OFFER.to_string(),
            amount: Some(21),
            message: None,
            request_id: Uuid::new_v4().to_string(),
        });
        store.save_prepared(&mut attempt).unwrap();
        store.begin_dispatch(&mut attempt).unwrap();
        store
            .record_payment(
                &mut attempt,
                WalletPaymentResult {
                    payment_id: "not-projected".to_string(),
                    status: "completed".to_string(),
                    status_message: String::new(),
                    amount: Some(21),
                    fees: 0,
                    created_at_ms: 0,
                    finalized_at_ms: Some(0),
                },
            )
            .unwrap();
        store.record_terminal_reuse(&attempt);

        let trace = crate::wallet::conformance::take_test_trace();
        check_wallet_trace(
            &trace,
            &WalletCheckerConfig::default()
                .require("prepare_generic")
                .require("begin_dispatch")
                .require("record_completed")
                .require("reuse_terminal"),
        )
        .expect("implementation trace must conform");
    }

    #[test]
    fn persisted_reconciliation_and_failure_execution_conforms() {
        use buzz_conformance_pkg::wallet::{check_wallet_trace, WalletCheckerConfig};

        let _ = crate::wallet::conformance::take_test_trace();
        let temp = tempfile::tempdir().unwrap();
        let store = SendAttemptStore::new(temp.path(), &"c".repeat(64));
        let mut attempt = SendAttempt::prepare(WalletSendRequest {
            destination: VALID_OFFER.to_string(),
            amount: Some(21),
            message: None,
            request_id: Uuid::new_v4().to_string(),
        });
        store.save_prepared(&mut attempt).unwrap();
        store.record_conflict(&attempt);
        store.begin_dispatch(&mut attempt).unwrap();
        store.record_reconcile(&attempt).unwrap();
        store
            .record_payment(
                &mut attempt,
                WalletPaymentResult {
                    payment_id: "not-projected".to_string(),
                    status: "pending".to_string(),
                    status_message: String::new(),
                    amount: Some(21),
                    fees: 0,
                    created_at_ms: 0,
                    finalized_at_ms: None,
                },
            )
            .unwrap();
        store.record_reconcile(&attempt).unwrap();
        store.fail_reconciliation(&mut attempt).unwrap();
        store.record_terminal_reuse(&attempt);

        let trace = crate::wallet::conformance::take_test_trace();
        check_wallet_trace(
            &trace,
            &WalletCheckerConfig::default()
                .require("reject_conflict")
                .require("reconcile")
                .require("record_pending")
                .require("record_failed")
                .require("reuse_terminal"),
        )
        .expect("reconciliation implementation trace must conform");
    }
}

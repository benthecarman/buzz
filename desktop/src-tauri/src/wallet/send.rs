use std::{
    io::Write,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use atomic_write_file::AtomicWriteFile;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::models::{WalletError, WalletPaymentResult, WalletSendRequest};

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
}

pub struct SendAttemptStore {
    directory: PathBuf,
}

impl SendAttemptStore {
    pub fn new(app_data_dir: &Path, payer_pubkey: &str) -> Self {
        Self {
            directory: app_data_dir
                .join("wallet")
                .join("send-attempts")
                .join(payer_pubkey),
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

    pub fn save(&self, attempt: &mut SendAttempt) -> Result<(), WalletError> {
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
        store.save(&mut attempt).unwrap();
        assert_eq!(
            store.load(&attempt.request.request_id).unwrap(),
            Some(attempt.clone())
        );
        assert_eq!(store.latest_pending().unwrap(), Some(attempt.request));
    }
}

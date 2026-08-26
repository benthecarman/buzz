//! Secret-free runtime trace emitter for durable wallet attempts.

use std::io::Write;
use std::sync::{Mutex, OnceLock};

use buzz_conformance_pkg::wallet::{
    WalletAbstractState, WalletAttemptId, WalletTraceAction, WalletTraceStep,
};
use sha2_10::{Digest, Sha256};

/// Persistence namespace for a wallet payment attempt.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WalletAttemptKind {
    /// A generic wallet send.
    GenericSend,
    /// An attributed profile zap.
    ProfileZap,
}

impl WalletAttemptKind {
    const fn label(self) -> &'static [u8] {
        match self {
            Self::GenericSend => b"generic_send",
            Self::ProfileZap => b"profile_zap",
        }
    }
}

/// Convert an identity-scoped request UUID to a stable opaque trace label.
pub fn attempt_id(payer_pubkey: &str, kind: WalletAttemptKind, raw_id: &str) -> WalletAttemptId {
    let mut hasher = Sha256::new();
    hasher.update(b"buzz-wallet-attempt-v1\0");
    hasher.update(kind.label());
    hasher.update(b"\0");
    hasher.update(payer_pubkey.as_bytes());
    hasher.update(b"\0");
    hasher.update(raw_id.as_bytes());
    let digest = hasher.finalize();
    WalletAttemptId(hex::encode(&digest[..16]))
}

/// Emit one step after durable persistence and before any following external
/// side effect. File output is opt-in; projection tests always collect steps.
pub fn record(
    payer_pubkey: &str,
    kind: WalletAttemptKind,
    raw_id: &str,
    action: WalletTraceAction,
    state_before: WalletAbstractState,
    state_after: WalletAbstractState,
) {
    let step = WalletTraceStep::new(
        attempt_id(payer_pubkey, kind, raw_id),
        action,
        state_before,
        state_after,
    );

    #[cfg(test)]
    TEST_TRACE.with(|trace| trace.borrow_mut().push(step.clone()));

    let Some(path) = std::env::var_os("BUZZ_WALLET_TRACE_PATH") else {
        return;
    };
    let Ok(_guard) = TRACE_LOCK.get_or_init(Mutex::default).lock() else {
        tracing::warn!("wallet trace sink lock is poisoned");
        return;
    };
    let Ok(mut file) = super::private_append_file(path) else {
        tracing::warn!("open BUZZ_WALLET_TRACE_PATH failed");
        return;
    };
    let Ok(mut line) = serde_json::to_vec(&step) else {
        tracing::warn!("serialize wallet trace step failed");
        return;
    };
    line.push(b'\n');
    if let Err(error) = file.write_all(&line).and_then(|()| file.flush()) {
        tracing::warn!(%error, "write wallet trace step failed");
    }
}

static TRACE_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

#[cfg(test)]
thread_local! {
    static TEST_TRACE: std::cell::RefCell<Vec<WalletTraceStep>> = const { std::cell::RefCell::new(Vec::new()) };
}

/// Clear and return trace steps produced on the current test thread.
#[cfg(test)]
pub fn take_test_trace() -> Vec<WalletTraceStep> {
    TEST_TRACE.with(|trace| std::mem::take(&mut *trace.borrow_mut()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn attempt_ids_are_scoped_by_payer_and_kind() {
        let raw_id = "84f55d93-05af-4c56-b7e6-4c13ef96dc13";
        let payer_a = "a".repeat(64);
        let payer_b = "b".repeat(64);
        let generic = attempt_id(&payer_a, WalletAttemptKind::GenericSend, raw_id);

        assert_ne!(
            generic,
            attempt_id(&payer_b, WalletAttemptKind::GenericSend, raw_id)
        );
        assert_ne!(
            generic,
            attempt_id(&payer_a, WalletAttemptKind::ProfileZap, raw_id)
        );
        assert_eq!(
            generic,
            attempt_id(&payer_a, WalletAttemptKind::GenericSend, raw_id)
        );
    }
}

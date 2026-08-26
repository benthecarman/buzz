use std::{
    collections::{HashMap, HashSet},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, OnceLock,
    },
};

use tauri::{AppHandle, Emitter, Manager};

use super::{app_data_dir, wallet_manager, zap_commands};
use crate::{
    app_state::AppState,
    wallet::{
        models::{WalletError, WalletIncomingPaymentEvent, WalletTransaction},
        provider::WalletProvider,
    },
};

const INCOMING_PAYMENT_EVENT: &str = "wallet-incoming-payment";
const INCOMING_PAYMENT_POLL_SECS: u64 = 5;
pub(super) static WALLET_POLLING_ENABLED: AtomicBool = AtomicBool::new(false);

#[derive(Default)]
struct IncomingPaymentTracker {
    completed_by_wallet: HashMap<String, HashSet<String>>,
}

impl IncomingPaymentTracker {
    fn observe(
        &mut self,
        wallet_pubkey: &str,
        transactions: &[WalletTransaction],
    ) -> Vec<WalletTransaction> {
        let completed = transactions
            .iter()
            .filter(|transaction| {
                transaction.direction == "inbound" && transaction.status == "completed"
            })
            .collect::<Vec<_>>();
        let Some(seen) = self.completed_by_wallet.get_mut(wallet_pubkey) else {
            self.completed_by_wallet.insert(
                wallet_pubkey.to_string(),
                completed
                    .into_iter()
                    .map(|transaction| transaction.id.clone())
                    .collect(),
            );
            return Vec::new();
        };

        completed
            .into_iter()
            .filter(|transaction| seen.insert(transaction.id.clone()))
            .cloned()
            .collect()
    }

    fn clear(&mut self) {
        self.completed_by_wallet.clear();
    }

    fn has_baseline(&self, wallet_pubkey: &str) -> bool {
        self.completed_by_wallet.contains_key(wallet_pubkey)
    }
}

fn incoming_payment_tracker() -> &'static tokio::sync::Mutex<IncomingPaymentTracker> {
    static TRACKER: OnceLock<tokio::sync::Mutex<IncomingPaymentTracker>> = OnceLock::new();
    TRACKER.get_or_init(Default::default)
}

pub(super) async fn reset_incoming_payment_tracker() {
    incoming_payment_tracker().lock().await.clear();
}

pub(super) async fn ensure_incoming_payment_baseline(
    wallet_pubkey: &str,
    provider: &Arc<dyn WalletProvider>,
) -> Result<(), WalletError> {
    if incoming_payment_tracker()
        .lock()
        .await
        .has_baseline(wallet_pubkey)
    {
        return Ok(());
    }
    provider.poll_updates().await?;
    let page = provider.transactions(None, 100, false).await?;
    incoming_payment_tracker()
        .lock()
        .await
        .observe(wallet_pubkey, &page.transactions);
    Ok(())
}

async fn poll_incoming_payments_once(app: &AppHandle, state: &AppState) -> Result<(), WalletError> {
    if !WALLET_POLLING_ENABLED.load(Ordering::Acquire) {
        return Ok(());
    }
    let keys = state.signing_keys().map_err(WalletError::unavailable)?;
    let wallet_pubkey = keys.public_key().to_hex();
    let provider = wallet_manager()
        .provider_for(&keys, &app_data_dir(app)?)
        .await?;
    provider.poll_updates().await?;
    let page = provider.transactions(None, 100, false).await?;
    let status = provider.status().await?;
    let incoming = incoming_payment_tracker()
        .lock()
        .await
        .observe(&wallet_pubkey, &page.transactions);
    if !WALLET_POLLING_ENABLED.load(Ordering::Acquire) {
        return Ok(());
    }
    for transaction in incoming {
        let event = WalletIncomingPaymentEvent {
            transaction,
            status: status.clone(),
            transactions: page.transactions.clone(),
        };
        if let Err(error) = app.emit(INCOMING_PAYMENT_EVENT, &event) {
            tracing::warn!(error = %error, "emit incoming wallet payment");
        }
    }
    Ok(())
}

/// Start wallet reconciliation for the lifetime of the application.
pub fn start_wallet_reconciler(app: AppHandle) {
    let zap_app = app.clone();
    tauri::async_runtime::spawn(async move {
        let mut consecutive_failures = 0u32;
        loop {
            let state = zap_app.state::<AppState>();
            match zap_commands::reconcile_wallet_background_once(&zap_app, &state).await {
                Ok(_) => consecutive_failures = 0,
                Err(error) => {
                    consecutive_failures = consecutive_failures.saturating_add(1);
                    tracing::warn!(
                        code = error.code,
                        error = %error.message,
                        "background wallet reconciliation failed"
                    );
                }
            }
            let multiplier = 1u64 << consecutive_failures.min(4);
            tokio::time::sleep(std::time::Duration::from_secs(
                15u64.saturating_mul(multiplier).min(5 * 60),
            ))
            .await;
        }
    });
    tauri::async_runtime::spawn(async move {
        let mut consecutive_failures = 0u32;
        loop {
            let state = app.state::<AppState>();
            if WALLET_POLLING_ENABLED.load(Ordering::Acquire) {
                match poll_incoming_payments_once(&app, &state).await {
                    Ok(()) => consecutive_failures = 0,
                    Err(error) => {
                        consecutive_failures = consecutive_failures.saturating_add(1);
                        tracing::warn!(
                            code = error.code,
                            error = %error.message,
                            "incoming wallet payment poll failed"
                        );
                    }
                }
            } else {
                consecutive_failures = 0;
            }
            let multiplier = 1u64 << consecutive_failures.min(4);
            tokio::time::sleep(std::time::Duration::from_secs(
                INCOMING_PAYMENT_POLL_SECS
                    .saturating_mul(multiplier)
                    .min(60),
            ))
            .await;
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wallet::models::{WalletStatus, WalletTransaction};

    fn transaction(id: &str, direction: &str, status: &str) -> WalletTransaction {
        WalletTransaction {
            id: id.to_string(),
            direction: direction.to_string(),
            status: status.to_string(),
            status_message: status.to_string(),
            amount: Some(21),
            fees: 0,
            note: None,
            payer_note: None,
            offer_id: None,
            payment_hash: None,
            created_at_ms: 1,
            finalized_at_ms: (status == "completed").then_some(2),
        }
    }

    #[test]
    fn baselines_then_emits_each_completed_inbound_once() {
        let mut tracker = IncomingPaymentTracker::default();
        let existing = transaction("existing", "inbound", "completed");
        assert!(tracker
            .observe("wallet", std::slice::from_ref(&existing))
            .is_empty());
        let pending = transaction("new", "inbound", "pending");
        let outbound = transaction("outbound", "outbound", "completed");
        assert!(tracker
            .observe("wallet", &[existing.clone(), pending, outbound])
            .is_empty());
        let completed = transaction("new", "inbound", "completed");
        assert_eq!(
            tracker.observe("wallet", &[existing.clone(), completed.clone()]),
            vec![completed.clone()]
        );
        assert!(tracker.observe("wallet", &[existing, completed]).is_empty());
    }

    #[test]
    fn uses_an_independent_baseline_per_wallet() {
        let mut tracker = IncomingPaymentTracker::default();
        let payment = transaction("same-provider-id", "inbound", "completed");
        assert!(tracker
            .observe("alice", std::slice::from_ref(&payment))
            .is_empty());
        assert!(tracker.observe("bob", &[payment]).is_empty());
    }

    #[test]
    fn incoming_event_contains_the_authoritative_snapshot() {
        let payment = transaction("new", "inbound", "completed");
        let event = WalletIncomingPaymentEvent {
            transaction: payment.clone(),
            status: WalletStatus {
                provider_name: "Lexe".to_string(),
                balance: 21,
                spendable_balance: 20,
                lightning_balance: 21,
                onchain_balance: 0,
            },
            transactions: vec![payment],
        };
        let json = serde_json::to_value(event).expect("event serializes");
        assert_eq!(json["transaction"]["id"], "new");
        assert_eq!(json["status"]["spendableBalance"], 20);
        assert_eq!(json["transactions"][0]["id"], "new");
    }
}

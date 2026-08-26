use serde::Serialize;

#[derive(Serialize)]
pub struct WalletDisabledError {
    code: &'static str,
    message: &'static str,
}

fn disabled() -> WalletDisabledError {
    WalletDisabledError {
        code: "wallet_unavailable",
        message: "this Buzz binary was built without the `bitcoin` feature",
    }
}

pub fn start_wallet_reconciler(_app: tauri::AppHandle) {}

macro_rules! disabled_async_command {
        ($name:ident ( $($argument:ident : $type:ty),* $(,)? ) -> $result:ty) => {
            #[tauri::command]
            pub async fn $name($($argument: $type),*) -> Result<$result, WalletDisabledError> {
                $(let _ = $argument;)*
                Err(disabled())
            }
        };
    }

disabled_async_command!(wallet_enable(relay_urls: Option<Vec<String>>) -> serde_json::Value);
disabled_async_command!(wallet_disable(relay_urls: Option<Vec<String>>) -> serde_json::Value);
disabled_async_command!(wallet_get_status() -> serde_json::Value);
disabled_async_command!(wallet_create_receive_request() -> serde_json::Value);
disabled_async_command!(wallet_refresh_offer(relay_urls: Option<Vec<String>>) -> serde_json::Value);
disabled_async_command!(wallet_analyze_destination(destination: String) -> serde_json::Value);
disabled_async_command!(wallet_get_pending_send() -> serde_json::Value);
disabled_async_command!(wallet_send(request: serde_json::Value) -> serde_json::Value);
disabled_async_command!(
    wallet_list_transactions(
        cursor: Option<String>,
        limit: Option<usize>,
        sync: Option<bool>,
    ) -> serde_json::Value
);
disabled_async_command!(wallet_set_polling_enabled(enabled: bool) -> ());
disabled_async_command!(
    wallet_get_recipient_offer(recipient_pubkey: String) -> serde_json::Value
);
disabled_async_command!(
    wallet_get_pending_profile_zap(
        recipient_pubkey: String,
        target_event_id: Option<String>,
    ) -> serde_json::Value
);
disabled_async_command!(
    wallet_send_profile_zap(request: serde_json::Value) -> serde_json::Value
);
#[tauri::command]
pub fn wallet_reveal_recovery_phrase() -> Result<String, WalletDisabledError> {
    Err(disabled())
}

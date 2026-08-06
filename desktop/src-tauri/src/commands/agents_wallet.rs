use nostr::Keys;
use tauri::AppHandle;

use crate::{app_state::AppState, managed_agents::CreateManagedAgentRequest};

pub(super) async fn provision_agent_offer(
    app: &AppHandle,
    state: &AppState,
    agent_keys: &Keys,
    agent_pubkey: &str,
    input: &CreateManagedAgentRequest,
) {
    #[cfg(not(feature = "bitcoin"))]
    let _ = (app, state, agent_keys, agent_pubkey, input);

    #[cfg(feature = "bitcoin")]
    if !input.wallet_enabled {
        return;
    }

    #[cfg(feature = "bitcoin")]
    match crate::commands::wallet::enabled::provision_new_managed_agent_offer(
        app,
        state,
        agent_keys,
        input.wallet_relay_urls.clone(),
    )
    .await
    {
        Ok(warnings) => {
            for warning in warnings {
                tracing::warn!(agent_pubkey = %agent_pubkey, warning, "managed-agent wallet offer warning");
            }
        }
        Err(error) => {
            tracing::warn!(agent_pubkey = %agent_pubkey, error, "publish managed-agent wallet offer");
        }
    }
}

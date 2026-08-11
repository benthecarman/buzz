//! Read-only diagnosis of the paid-runtime mint decision for a live agent.
use buzz_acp::paid_runtime::{diagnose_scopes, PaidRuntimeTerms};
use buzz_acp::relay::RestClient;
use nostr::Keys;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let keys = Keys::parse(&std::env::var("BUZZ_PRIVATE_KEY")?)?;
    let ws = std::env::var("BUZZ_RELAY_URL")?;
    let base = ws.replace("wss://", "https://").replace("ws://", "http://");
    let rest = RestClient {
        http: reqwest::Client::new(),
        base_url: base,
        keys: keys.clone(),
        auth_tag_json: std::env::var("BUZZ_AUTH_TAG").ok(),
    };
    let terms = PaidRuntimeTerms {
        keys,
        respond_to: buzz_acp::config::RespondTo::Anyone,
        respond_to_allowlist: Default::default(),
        priced: true,
    };
    for line in diagnose_scopes(&terms, &rest).await? {
        println!("{line}");
    }
    Ok(())
}

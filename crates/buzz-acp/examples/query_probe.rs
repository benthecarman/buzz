//! Diagnostic: replay the minter's exact relay reads as a live agent identity.
//! Run with BUZZ_PRIVATE_KEY / BUZZ_AUTH_TAG / BUZZ_RELAY_URL set.
use buzz_acp::relay::RestClient;
use nostr::{Alphabet, Keys, Kind, SingleLetterTag};

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
    let agent = keys.public_key();
    println!("agent: {}", agent.to_hex());
    let channel = "e8b2198a-c7f1-4a44-848c-8c734c1f6869";
    let d = SingleLetterTag::lowercase(Alphabet::D);

    let deposits = rest
        .query(&[nostr::Filter::new().author(agent).kind(Kind::Custom(44210))])
        .await?;
    println!(
        "deposit scan: {} rows",
        deposits.as_array().map_or(0, Vec::len)
    );

    let meta = rest
        .query(&[nostr::Filter::new()
            .kind(Kind::Custom(39000))
            .custom_tags(d, [channel])])
        .await?;
    println!("39000 #d: {} rows", meta.as_array().map_or(0, Vec::len));

    let members = rest
        .query(&[nostr::Filter::new()
            .kind(Kind::Custom(39002))
            .custom_tags(d, [channel])])
        .await?;
    println!("39002 #d: {} rows", members.as_array().map_or(0, Vec::len));

    let p = SingleLetterTag::lowercase(Alphabet::P);
    let payer = "7dbc4bf4f47e0dd8f1294817e38864ee7df84a91bbfa581d4fbeb0f7b20a81d0";
    let with_p = rest
        .query(&[nostr::Filter::new()
            .author(agent)
            .kind(Kind::Custom(44210))
            .custom_tags(p, [payer])])
        .await?;
    println!(
        "44210 authors+#p (no #h): {} rows",
        with_p.as_array().map_or(0, Vec::len)
    );

    let h = SingleLetterTag::lowercase(Alphabet::H);
    let with_h = rest
        .query(&[nostr::Filter::new()
            .author(agent)
            .kind(Kind::Custom(44210))
            .custom_tags(p, [payer])
            .custom_tags(h, [channel])])
        .await?;
    println!(
        "44210 authors+#p+#h: {} rows",
        with_h.as_array().map_or(0, Vec::len)
    );
    Ok(())
}

//! Command-line entrypoint for wallet-offer JSONL replay.

use buzz_conformance::wallet_offer::{check_offer_jsonl, OfferCheckerConfig};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let path = args
        .next()
        .ok_or("usage: check-wallet-offer-trace <trace.jsonl> [required_action ...]")?;
    let jsonl = std::fs::read_to_string(&path)?;
    let config = args.fold(OfferCheckerConfig::default(), |config, action| {
        config.require(&action)
    });
    check_offer_jsonl(&jsonl, &config)?;
    println!("accepted wallet offer trace: {path}");
    Ok(())
}

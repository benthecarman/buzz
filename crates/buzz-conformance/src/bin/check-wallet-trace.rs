//! Command-line entrypoint for wallet JSONL replay.

use std::collections::BTreeSet;

use buzz_conformance::wallet::{check_wallet_jsonl, WalletCheckerConfig};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let path = args
        .next()
        .ok_or("usage: check-wallet-trace <trace.jsonl> [required_action ...]")?;
    let required_critical_actions: BTreeSet<String> = args.collect();
    let trace = std::fs::read_to_string(&path)?;
    check_wallet_jsonl(
        &trace,
        &WalletCheckerConfig {
            required_critical_actions,
        },
    )?;
    println!("accepted wallet trace: {path}");
    Ok(())
}

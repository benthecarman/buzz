use std::path::Path;

use buzz_conformance::wallet::{check_wallet_jsonl, WalletCheckError, WalletCheckerConfig};

fn fixture(name: &str) -> String {
    std::fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/wallet")
            .join(name),
    )
    .expect("wallet trace fixture")
}

#[test]
fn positive_runtime_trace_is_accepted() {
    check_wallet_jsonl(
        &fixture("good_generic.jsonl"),
        &WalletCheckerConfig::default()
            .require("prepare_generic")
            .require("begin_dispatch")
            .require("record_completed")
            .require("reuse_terminal"),
    )
    .expect("positive fixture");
}

#[test]
fn forbidden_runtime_trace_is_rejected() {
    assert!(matches!(
        check_wallet_jsonl(
            &fixture("bad_complete_without_dispatch.jsonl"),
            &WalletCheckerConfig::default()
        ),
        Err(WalletCheckError::IllegalTransition { step: 1, .. })
    ));
}

#[test]
fn unknown_critical_action_fixture_fails_closed() {
    assert!(matches!(
        check_wallet_jsonl(
            &fixture("bad_unknown_action.jsonl"),
            &WalletCheckerConfig::default()
        ),
        Err(WalletCheckError::CoverageBreach { .. })
    ));
}

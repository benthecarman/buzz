use std::path::Path;

use buzz_conformance::wallet_offer::{check_offer_jsonl, OfferCheckError, OfferCheckerConfig};

fn fixture(name: &str) -> String {
    std::fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/wallet_offer")
            .join(name),
    )
    .expect("wallet offer trace fixture")
}

#[test]
fn complete_offer_fanout_fixture_is_accepted() {
    check_offer_jsonl(
        &fixture("good_announcement.jsonl"),
        &OfferCheckerConfig::default()
            .require("begin_announcement")
            .require("relay_result")
            .require("finish_announcement"),
    )
    .expect("positive fixture");
}

#[test]
fn early_finish_fixture_is_rejected() {
    assert!(matches!(
        check_offer_jsonl(
            &fixture("bad_finish_before_fanout.jsonl"),
            &OfferCheckerConfig::default()
        ),
        Err(OfferCheckError::IllegalTransition { step: 1, .. })
    ));
}

#[test]
fn duplicate_cross_identity_offer_fixture_is_rejected() {
    assert!(matches!(
        check_offer_jsonl(
            &fixture("bad_duplicate_offer.jsonl"),
            &OfferCheckerConfig::default()
        ),
        Err(OfferCheckError::IllegalTransition { step: 1, .. })
    ));
}

#[test]
fn agent_wallet_offer_fixture_is_rejected() {
    assert!(matches!(
        check_offer_jsonl(
            &fixture("bad_agent_wallet_issuer.jsonl"),
            &OfferCheckerConfig::default()
        ),
        Err(OfferCheckError::IllegalTransition { step: 0, .. })
    ));
}

#[test]
fn unknown_offer_action_fails_closed() {
    assert!(matches!(
        check_offer_jsonl(
            &fixture("bad_unknown_action.jsonl"),
            &OfferCheckerConfig::default()
        ),
        Err(OfferCheckError::CoverageBreach { .. })
    ));
}

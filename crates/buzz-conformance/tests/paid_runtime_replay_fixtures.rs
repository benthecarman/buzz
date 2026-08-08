use std::path::Path;

use buzz_conformance::paid_agent_runtime::{
    check_runtime_jsonl, RuntimeCheckError, RuntimeCheckerConfig,
};

fn fixture(name: &str) -> String {
    std::fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/paid_runtime")
            .join(name),
    )
    .expect("paid-runtime fixture")
}

#[test]
fn complete_runtime_lifecycle_is_accepted() {
    check_runtime_jsonl(
        &fixture("good.jsonl"),
        &RuntimeCheckerConfig::default()
            .require("payment_settled")
            .require("credit_deposited")
            .require("runtime_reserved")
            .require("invocation_dispatched")
            .require("meter_started")
            .require("reservation_settled")
            .require("duplicate_reused"),
    )
    .expect("positive fixture");
}

#[test]
fn adversarial_runtime_fixtures_fail_closed() {
    for name in [
        "bad_credit_without_payment.jsonl",
        "bad_overspend.jsonl",
        "bad_meter_outside_prompt.jsonl",
        "bad_meter_without_dispatch.jsonl",
        "bad_duplicate_dispatch.jsonl",
        "bad_cross_community_bind.jsonl",
    ] {
        assert!(matches!(
            check_runtime_jsonl(&fixture(name), &RuntimeCheckerConfig::default()),
            Err(RuntimeCheckError::IllegalTransition { .. })
        ));
    }
}

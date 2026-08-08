use super::*;
use crate::wallet::VALID_OFFER;

fn recipient(keys: &Keys) -> WalletRecipientOffer {
    let event = EventBuilder::new(Kind::Custom(KIND_BOLT12_OFFER as u16), "")
        .tag(Tag::parse(["offer", VALID_OFFER]).unwrap())
        .sign_with_keys(keys)
        .unwrap();
    recipient_offer(&event, &keys.public_key().to_hex()).unwrap()
}

#[test]
fn withdrawal_announcement_validates_but_yields_no_offer() {
    let keys = Keys::generate();
    let withdrawal = EventBuilder::new(Kind::Custom(KIND_BOLT12_OFFER as u16), "")
        .sign_with_keys(&keys)
        .unwrap();
    assert!(validate_offer_event(&withdrawal, &keys.public_key().to_hex()).is_ok());
    assert_eq!(
        recipient_offer(&withdrawal, &keys.public_key().to_hex())
            .unwrap_err()
            .code,
        "offer_invalid"
    );
}

#[test]
fn persisted_profile_execution_emits_a_model_accepted_trace() {
    use buzz_conformance_pkg::wallet::{check_wallet_trace, WalletCheckerConfig};

    let _ = crate::wallet::conformance::take_test_trace();
    let temp = tempfile::tempdir().unwrap();
    let payer = Keys::generate();
    let recipient_keys = Keys::generate();
    let mut attempt = ZapAttempt::prepare(
        Uuid::new_v4().to_string(),
        recipient(&recipient_keys),
        21,
        None,
        None,
        None,
        &payer,
    )
    .unwrap();
    let store = ZapAttemptStore::new(temp.path(), &payer.public_key().to_hex());
    store.save_prepared(&mut attempt).unwrap();
    store.begin_dispatch(&mut attempt).unwrap();
    store.record_reconcile(&attempt).unwrap();
    store
        .record_payment(
            &mut attempt,
            WalletPaymentResult {
                payment_id: "not-projected".to_string(),
                status: "completed".to_string(),
                status_message: String::new(),
                amount: Some(21),
                fees: 0,
                created_at_ms: 0,
                finalized_at_ms: Some(0),
            },
        )
        .unwrap();

    let trace = crate::wallet::conformance::take_test_trace();
    check_wallet_trace(
        &trace,
        &WalletCheckerConfig::default()
            .require("prepare_profile")
            .require("begin_dispatch")
            .require("reconcile")
            .require("record_paid_without_proof"),
    )
    .expect("implementation trace must conform");
}

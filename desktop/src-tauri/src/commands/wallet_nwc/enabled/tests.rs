use buzz_core_pkg::nwc::{NwcPayParams, NwcPayRequest};

use super::{payment_response_result, validate_payer_note, visible_balance_msats};
use crate::wallet::{
    models::{WalletNwcRequest, WalletPaymentResult, WalletPaymentStatus},
    VALID_INVOICE,
};

#[test]
fn rejects_a_payer_note_for_bolt11() {
    let request = NwcPayRequest {
        method: "pay".into(),
        params: NwcPayParams {
            payment: format!("bitcoin:?lightning={VALID_INVOICE}"),
            amount: Some(100_000),
            payer_note: Some("not supported by BOLT11".into()),
            metadata: Default::default(),
        },
    };
    assert_eq!(
        validate_payer_note(&request, "bolt11").unwrap_err().code,
        "invalid_payment"
    );
}

#[test]
fn balance_is_limited_by_budget_and_wallet() {
    assert_eq!(visible_balance_msats(50, 100), 50_000);
    assert_eq!(visible_balance_msats(100, 50), 50_000);
    assert_eq!(visible_balance_msats(u64::MAX, u64::MAX), u64::MAX);
}

#[test]
fn successful_response_keeps_payment_artifacts() {
    let request = WalletNwcRequest {
        event_id: "event".into(),
        expires_at_ms: 60_000,
        agent_pubkey: "agent".into(),
        agent_name: "Amber Heron".into(),
        request_type: "payment".into(),
        instruction_type: "bolt12".into(),
        recipient_pubkey: None,
        amount: 21,
        comment: String::new(),
        destination: "lno1example".into(),
        payer_note: None,
        request_id: "request".into(),
    };
    let result = payment_response_result(
        &request,
        WalletPaymentResult {
            payment_id: "provider-id".into(),
            status: WalletPaymentStatus::Completed,
            status_message: String::new(),
            preimage: Some("11".repeat(32)),
            payer_proof: Some("lnp1proof".into()),
            txid: Some("22".repeat(32)),
            amount: Some(21),
            fees: 1,
            created_at_ms: 1_000,
            finalized_at_ms: Some(2_000),
        },
    );
    assert_eq!(result.transaction_id, "provider-id");
    assert_eq!(result.preimage, Some("11".repeat(32)));
    assert_eq!(result.payer_proof.as_deref(), Some("lnp1proof"));
    assert_eq!(result.txid, Some("22".repeat(32)));
}

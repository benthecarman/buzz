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
fn intent_matches_protocol_shape_and_uses_nip_b1_note() {
    let payer = Keys::generate();
    let recipient_keys = Keys::generate();
    let attempt = ZapAttempt::prepare(
        Uuid::new_v4().to_string(),
        recipient(&recipient_keys),
        21,
        Some("great work".to_string()),
        ZapTarget {
            event_id: None,
            event_kind: None,
            channel_id: None,
            lease_id: None,
        },
        &payer,
    )
    .unwrap();
    let intent = Event::from_json(&attempt.intent_event_json).unwrap();
    assert_eq!(intent.kind, Kind::Custom(KIND_BOLT12_ZAP_INTENT as u16));
    assert_eq!(intent.content, "great work");
    assert!(intent
        .tags
        .iter()
        .any(|tag| tag.as_slice() == ["amount", "21000"]));
    assert_eq!(
        attempt.payer_note,
        format!("nostr:nipB1:{}", intent.id.to_hex())
    );
    let zap_id = intent
        .tags
        .iter()
        .find_map(|tag| {
            let parts = tag.as_slice();
            (parts.first().map(String::as_str) == Some("zap_id"))
                .then(|| parts.get(1))
                .flatten()
        })
        .unwrap();
    assert_eq!(zap_id.len(), 32);
    assert!(zap_id
        .chars()
        .all(|character| character.is_ascii_hexdigit()));
    assert_eq!(zap_id, &zap_id.to_ascii_lowercase());
}

#[test]
fn event_intent_binds_message_id_and_kind() {
    let payer = Keys::generate();
    let recipient_keys = Keys::generate();
    let target_event_id = "ab".repeat(32);
    let attempt = ZapAttempt::prepare(
        Uuid::new_v4().to_string(),
        recipient(&recipient_keys),
        21,
        None,
        ZapTarget {
            event_id: Some(target_event_id.clone()),
            event_kind: Some(40_002),
            channel_id: None,
            lease_id: None,
        },
        &payer,
    )
    .unwrap();
    let intent = Event::from_json(&attempt.intent_event_json).unwrap();
    assert!(intent
        .tags
        .iter()
        .any(|tag| tag.as_slice() == ["e", target_event_id.as_str()]));
    assert!(intent
        .tags
        .iter()
        .any(|tag| tag.as_slice() == ["k", "40002"]));
}

#[test]
fn hosted_agent_intent_binds_plan_channel_and_lease() {
    let payer = Keys::generate();
    let recipient_keys = Keys::generate();
    let target_event_id = "ab".repeat(32);
    let lease_id = Uuid::new_v4().to_string();
    let attempt = ZapAttempt::prepare(
        Uuid::new_v4().to_string(),
        recipient(&recipient_keys),
        500,
        None,
        ZapTarget {
            event_id: Some(target_event_id.clone()),
            event_kind: Some(40_002),
            channel_id: Some("channel-id".to_string()),
            lease_id: Some(lease_id.clone()),
        },
        &payer,
    )
    .unwrap();
    let intent = Event::from_json(&attempt.intent_event_json).unwrap();
    assert!(intent
        .tags
        .iter()
        .any(|tag| tag.as_slice() == ["e", target_event_id.as_str()]));
    assert!(intent
        .tags
        .iter()
        .any(|tag| tag.as_slice() == ["h", "channel-id"]));
    assert!(intent
        .tags
        .iter()
        .any(|tag| tag.as_slice() == ["lease", lease_id.as_str()]));
    assert!(intent
        .tags
        .iter()
        .any(|tag| tag.as_slice() == ["k", "40002"]));
}

#[test]
fn parses_only_canonical_lowercase_offers() {
    assert!(build_offer_announcement(VALID_OFFER).is_ok());
    assert!(build_offer_announcement(&VALID_OFFER.to_ascii_uppercase()).is_err());
    assert!(build_offer_announcement(&VALID_OFFER[..VALID_OFFER.len() - 1]).is_err());
    assert!(build_offer_announcement(&format!("{VALID_OFFER} ")).is_err());
}

#[test]
fn rejects_announcement_when_any_offer_is_not_canonical() {
    let keys = Keys::generate();
    let event = EventBuilder::new(Kind::Custom(KIND_BOLT12_OFFER as u16), "")
        .tags([
            Tag::parse(["offer", VALID_OFFER]).unwrap(),
            Tag::parse(["offer", &VALID_OFFER.to_ascii_uppercase()]).unwrap(),
        ])
        .sign_with_keys(&keys)
        .unwrap();
    assert_eq!(
        recipient_offer(&event, &keys.public_key().to_hex())
            .unwrap_err()
            .code,
        "offer_invalid"
    );
}

#[test]
fn rejects_zero_or_overflowing_amount() {
    assert!(amount_msats(0).is_err());
    assert!(amount_msats(u64::MAX).is_err());
}

#[test]
fn attempt_store_round_trips_and_restores_pending_draft() {
    let temp = tempfile::tempdir().unwrap();
    let payer = Keys::generate();
    let recipient_keys = Keys::generate();
    let mut attempt = ZapAttempt::prepare(
        Uuid::new_v4().to_string(),
        recipient(&recipient_keys),
        21,
        None,
        ZapTarget {
            event_id: None,
            event_kind: None,
            channel_id: None,
            lease_id: None,
        },
        &payer,
    )
    .unwrap();
    let store = ZapAttemptStore::new(temp.path(), &payer.public_key().to_hex());
    store.save_prepared(&mut attempt).unwrap();
    assert_eq!(
        store.load(&attempt.idempotency_key).unwrap(),
        Some(attempt.clone())
    );
    assert_eq!(
        store
            .pending_for_recipient(&attempt.recipient_pubkey, None, "https://relay.example",)
            .unwrap()
            .unwrap()
            .idempotency_key,
        attempt.idempotency_key
    );
}

#[test]
fn attempt_store_rejects_fields_that_diverge_from_the_signed_intent() {
    let temp = tempfile::tempdir().unwrap();
    let payer = Keys::generate();
    let recipient_keys = Keys::generate();
    let lease_id = Uuid::new_v4().to_string();
    let attempt = ZapAttempt::prepare(
        Uuid::new_v4().to_string(),
        recipient(&recipient_keys),
        21,
        Some("signed comment".to_string()),
        ZapTarget {
            event_id: Some("ab".repeat(32)),
            event_kind: Some(40_002),
            channel_id: Some("signed-channel".to_string()),
            lease_id: Some(lease_id),
        },
        &payer,
    )
    .unwrap();
    let store = ZapAttemptStore::new(temp.path(), &payer.public_key().to_hex());

    for case in 0..6 {
        let mut tampered = attempt.clone();
        tampered.idempotency_key = Uuid::new_v4().to_string();
        match case {
            0 => tampered.target_event_id = Some("cd".repeat(32)),
            1 => tampered.target_event_kind = Some(1),
            2 => tampered.channel_id = Some("other-channel".to_string()),
            3 => tampered.lease_id = Some(Uuid::new_v4().to_string()),
            4 => tampered.offer = "not-the-signed-offer".to_string(),
            5 => tampered.comment = Some("other comment".to_string()),
            _ => unreachable!(),
        }
        store.save(&mut tampered).unwrap();
        assert!(
            store.load(&tampered.idempotency_key).is_err(),
            "case {case}"
        );
    }
}

#[test]
fn zap_attempt_scan_keeps_valid_entries_after_corruption() {
    let temp = tempfile::tempdir().unwrap();
    let payer = Keys::generate();
    let recipient_keys = Keys::generate();
    let mut attempt = ZapAttempt::prepare(
        Uuid::new_v4().to_string(),
        recipient(&recipient_keys),
        21,
        None,
        ZapTarget {
            event_id: None,
            event_kind: None,
            channel_id: None,
            lease_id: None,
        },
        &payer,
    )
    .unwrap();
    let store = ZapAttemptStore::new(temp.path(), &payer.public_key().to_hex());
    store.save_prepared(&mut attempt).unwrap();
    std::fs::write(
        store.directory.join(format!("{}.json", Uuid::new_v4())),
        b"not json",
    )
    .unwrap();

    assert_eq!(
        store
            .pending_for_recipient(&attempt.recipient_pubkey, None, "https://relay.example")
            .unwrap()
            .unwrap()
            .idempotency_key,
        attempt.idempotency_key
    );
}

#[test]
fn settled_message_zap_builds_a_relay_proof() {
    let temp = tempfile::tempdir().unwrap();
    let payer = Keys::generate();
    let recipient_keys = Keys::generate();
    let target_event_id = "ab".repeat(32);
    let mut attempt = ZapAttempt::prepare(
        Uuid::new_v4().to_string(),
        recipient(&recipient_keys),
        21,
        Some("great work".to_string()),
        ZapTarget {
            event_id: Some(target_event_id.clone()),
            event_kind: Some(40_002),
            channel_id: None,
            lease_id: None,
        },
        &payer,
    )
    .unwrap();
    let store = ZapAttemptStore::new(temp.path(), &payer.public_key().to_hex());
    store.save_prepared(&mut attempt).unwrap();
    store.begin_dispatch(&mut attempt).unwrap();
    store
        .record_payment(
            &mut attempt,
            WalletPaymentResult {
                payment_id: "payment".to_string(),
                status: WalletPaymentStatus::Completed,
                status_message: String::new(),
                amount: Some(21),
                fees: 0,
                created_at_ms: 100,
                finalized_at_ms: Some(200),
            },
        )
        .unwrap();

    store
        .bind_relay_if_missing(&mut attempt, "https://relay.example/")
        .unwrap();
    assert_eq!(
        store
            .unpublished_proofs_for_relay("https://relay.example")
            .unwrap(),
        vec![attempt.clone()]
    );
    assert!(store
        .unpublished_proofs_for_relay("https://other.example")
        .unwrap()
        .is_empty());

    assert_eq!(
        store
            .pending_for_recipient(
                &attempt.recipient_pubkey,
                Some(&target_event_id),
                "https://relay.example",
            )
            .unwrap()
            .unwrap()
            .idempotency_key,
        attempt.idempotency_key
    );

    let proof = store
        .prepare_placeholder_proof(&mut attempt, &payer, Some("channel-id"))
        .unwrap();
    assert_eq!(proof.kind, Kind::Custom(KIND_BOLT12_ZAP as u16));
    assert!(proof
        .tags
        .iter()
        .any(|tag| tag.as_slice() == ["proof", PLACEHOLDER_PAYER_PROOF]));
    assert!(proof
        .tags
        .iter()
        .any(|tag| tag.as_slice() == ["h", "channel-id"]));
    assert!(proof
        .tags
        .iter()
        .any(|tag| tag.as_slice() == ["e", target_event_id.as_str()]));
    assert!(proof
        .tags
        .iter()
        .any(|tag| tag.as_slice() == ["amount", "21000"]));
    let persisted = store
        .prepare_placeholder_proof(&mut attempt, &payer, None)
        .unwrap();
    assert_eq!(persisted.id, proof.id);

    let mut abandoned = attempt.clone();
    abandoned.idempotency_key = Uuid::new_v4().to_string();
    store.save(&mut abandoned).unwrap();
    store.abandon_proof(&mut abandoned).unwrap();
    assert!(store
        .unpublished_proofs_for_relay("https://relay.example")
        .unwrap()
        .iter()
        .all(|candidate| candidate.idempotency_key != abandoned.idempotency_key));
    assert!(store
        .pending_for_recipient(
            &abandoned.recipient_pubkey,
            Some(&target_event_id),
            "https://relay.example",
        )
        .unwrap()
        .is_some_and(|candidate| candidate.idempotency_key != abandoned.idempotency_key));

    store.mark_proof_published(&mut attempt).unwrap();
    assert!(store
        .unpublished_proofs_for_relay("https://relay.example")
        .unwrap()
        .is_empty());
    assert!(store
        .pending_for_recipient(
            &attempt.recipient_pubkey,
            Some(&target_event_id),
            "https://relay.example",
        )
        .unwrap()
        .is_none());
    assert!(attempt.result().unwrap().proof_published);
}

#[test]
fn paying_attempts_are_relay_scoped_for_background_reconciliation() {
    let temp = tempfile::tempdir().unwrap();
    let payer = Keys::generate();
    let recipient_keys = Keys::generate();
    let mut attempt = ZapAttempt::prepare(
        Uuid::new_v4().to_string(),
        recipient(&recipient_keys),
        21,
        None,
        ZapTarget {
            event_id: None,
            event_kind: None,
            channel_id: None,
            lease_id: None,
        },
        &payer,
    )
    .unwrap();
    attempt.relay_url = Some("https://relay.example/".to_string());
    let store = ZapAttemptStore::new(temp.path(), &payer.public_key().to_hex());
    store.save_prepared(&mut attempt).unwrap();
    store.begin_dispatch(&mut attempt).unwrap();
    store
        .record_payment(
            &mut attempt,
            WalletPaymentResult {
                payment_id: "payment".to_string(),
                status: WalletPaymentStatus::Pending,
                status_message: String::new(),
                amount: Some(21),
                fees: 0,
                created_at_ms: 100,
                finalized_at_ms: None,
            },
        )
        .unwrap();

    assert_eq!(
        store
            .paying_attempts_for_relay("https://relay.example")
            .unwrap(),
        vec![attempt]
    );
    assert!(store
        .paying_attempts_for_relay("https://other.example")
        .unwrap()
        .is_empty());
}

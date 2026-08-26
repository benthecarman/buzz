use super::*;

use crate::wallet::VALID_OFFER;

const TIMESTAMPED_PROOF: &str = "lnp1tqssxkl9a9rcyzt8f2twvrclqdlkzaj5plgqr7sav355wux9dfmsn3pv5szx4psnpk5zq00s6e7vymzt2ds0kgtw5pr6h787ysnwjw3wu5dnzfx97mymqd5c4gp2gy9syyptkk94lm99qhr5ahqqpkpg9lz4deg6zqj0erna0etvd7y8chydtuhsgq7ded94zlrpvt89k2rml4ulg8qp4lj25rgq8jxx66lyett8d2t0u40q8rdcuec6jsdjznu4kxljg5a2xyfn0c5nvllqhsfc077k33xh79q0wnkl7p53rt5wdx5heuhv65yz2st5zedrh0d34w2kw0uwfy5am5xz5uzyvz37fkknsdy0n2kn65ej5jxpdrae7wappc0xmx7qhk7cgr7s86fq9skzctpv9skzctpv9skzctpv9skzctpv9skzctpv9skzctpv9sk06ql2qsqsyk26l5p7kcp39gkgl3dvh384c64425qrhfdf25m2vklmxc8ys785gxrwhx869jeak0q0u6yn7kq9j79nnvgzplc4wt9dg7zsa4nt5pmszslxk3x50lrw26z0azerm45shxk2d4s3k623ve8lq6wy60fq3w59erhmk6n6l5p7egrc87jvfx62msdekqde8w6rahk5fhu3k5xv8plyegs0we84x0slee27gxzus5hczl7pvsyz0pudg2tz9uegx9mjyg7rzg0cgaxdqhamprruks5zjws2xvanfgr7qchk6ur3900gxcghlzsm00mc83xrfs0gx6eulceyzwh3c29fgcwrqt2rrkdq2meyw5093uzj00zyjew22kvmrzmsf3qgxtl99xy6xrtnfth426wwy62qatlnxtryw8j7hf2uk6laq0kskar9wd6zq6tww3jkuaq";

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
fn skips_noncanonical_offers_in_an_announcement() {
    let keys = Keys::generate();
    let event = EventBuilder::new(Kind::Custom(KIND_BOLT12_OFFER as u16), "")
        .tags([
            Tag::parse(["offer", &VALID_OFFER.to_ascii_uppercase()]).unwrap(),
            Tag::parse(["offer", VALID_OFFER]).unwrap(),
        ])
        .sign_with_keys(&keys)
        .unwrap();
    assert_eq!(
        recipient_offer(&event, &keys.public_key().to_hex())
            .unwrap()
            .offer,
        VALID_OFFER
    );
}

#[test]
fn rejects_announcement_without_a_canonical_offer() {
    let keys = Keys::generate();
    let event = EventBuilder::new(Kind::Custom(KIND_BOLT12_OFFER as u16), "")
        .tag(Tag::parse(["offer", "invalid-offer"]).unwrap())
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
    let payer_proof = TIMESTAMPED_PROOF.to_string();
    let mut attempt = ZapAttempt::prepare(
        Uuid::new_v4().to_string(),
        recipient(&recipient_keys),
        21,
        Some("great work".to_string()),
        ZapTarget {
            event_id: Some(target_event_id.clone()),
            event_kind: Some(40_002),
            channel_id: Some("channel-id".to_string()),
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
                preimage: Some("11".repeat(32)),
                payer_proof: Some(payer_proof.clone()),
                txid: None,
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

    let before_proof = nostr::Timestamp::now().as_secs();
    let proof = store
        .prepare_proof(&mut attempt, &payer, Some("channel-id"))
        .unwrap();
    let after_proof = nostr::Timestamp::now().as_secs();
    assert_eq!(proof.kind, Kind::Custom(KIND_BOLT12_ZAP as u16));
    assert!(proof
        .tags
        .iter()
        .any(|tag| tag.as_slice() == ["proof", payer_proof.as_str()]));
    assert!(proof
        .tags
        .iter()
        .any(|tag| tag.as_slice() == ["h", "channel-id"]));
    assert!(proof.created_at.as_secs() >= before_proof);
    assert!(proof.created_at.as_secs() <= after_proof);
    assert!(proof
        .tags
        .iter()
        .any(|tag| tag.as_slice() == ["e", target_event_id.as_str()]));
    assert!(proof
        .tags
        .iter()
        .any(|tag| tag.as_slice() == ["amount", "21000"]));
    let persisted = store.prepare_proof(&mut attempt, &payer, None).unwrap();
    assert_eq!(persisted.id, proof.id);

    let mut formerly_abandoned = attempt.clone();
    formerly_abandoned.idempotency_key = Uuid::new_v4().to_string();
    formerly_abandoned.proof_retry_abandoned = true;
    store.save(&mut formerly_abandoned).unwrap();
    assert!(store
        .unpublished_proofs_for_relay("https://relay.example")
        .unwrap()
        .iter()
        .any(|candidate| candidate.idempotency_key == formerly_abandoned.idempotency_key));

    store.mark_proof_published(&mut attempt).unwrap();
    let unpublished = store
        .unpublished_proofs_for_relay("https://relay.example")
        .unwrap();
    assert_eq!(unpublished.len(), 1);
    assert_eq!(
        unpublished[0].idempotency_key,
        formerly_abandoned.idempotency_key
    );
    let pending = store
        .pending_for_recipient(
            &attempt.recipient_pubkey,
            Some(&target_event_id),
            "https://relay.example",
        )
        .unwrap()
        .expect("legacy recovery remains pending");
    assert_eq!(pending.idempotency_key, formerly_abandoned.idempotency_key);
    assert!(attempt.result().unwrap().proof_published);
}

#[test]
fn settled_legacy_message_zap_builds_proof_without_channel() {
    let temp = tempfile::tempdir().unwrap();
    let payer = Keys::generate();
    let recipient_keys = Keys::generate();
    let target_event_id = "ab".repeat(32);
    let mut attempt = ZapAttempt::prepare(
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
                preimage: Some("11".repeat(32)),
                payer_proof: Some(TIMESTAMPED_PROOF.to_string()),
                txid: None,
                amount: Some(21),
                fees: 0,
                created_at_ms: 100,
                finalized_at_ms: Some(200),
            },
        )
        .unwrap();

    let proof = store
        .prepare_proof(&mut attempt, &payer, Some("channel-id"))
        .unwrap();

    assert!(proof
        .tags
        .iter()
        .any(|tag| tag.as_slice() == ["e", target_event_id.as_str()]));
    assert!(!proof
        .tags
        .iter()
        .any(|tag| tag.as_slice().first().map(String::as_str) == Some("h")));
}

#[test]
fn settled_channel_bound_zap_rejects_a_different_channel() {
    let temp = tempfile::tempdir().unwrap();
    let payer = Keys::generate();
    let recipient_keys = Keys::generate();
    let mut attempt = ZapAttempt::prepare(
        Uuid::new_v4().to_string(),
        recipient(&recipient_keys),
        21,
        None,
        ZapTarget {
            event_id: Some("ab".repeat(32)),
            event_kind: Some(40_002),
            channel_id: Some("signed-channel".to_string()),
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
                preimage: Some("11".repeat(32)),
                payer_proof: Some(TIMESTAMPED_PROOF.to_string()),
                txid: None,
                amount: Some(21),
                fees: 0,
                created_at_ms: 100,
                finalized_at_ms: Some(200),
            },
        )
        .unwrap();

    let error = store
        .prepare_proof(&mut attempt, &payer, Some("different-channel"))
        .unwrap_err();

    assert_eq!(error.code, "invalid_zap");
    assert_eq!(
        error.message,
        "The channel must be signed into the zap intent"
    );
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
                preimage: None,
                payer_proof: None,
                txid: None,
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

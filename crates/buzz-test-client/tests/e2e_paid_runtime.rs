//! E2E: the prepaid paid-agent-runtime protocol with the wallet mocked out.
//!
//! Requires: relay running at localhost:3000 with all migrations applied,
//! including 0030 (the reservation-claim trigger).
//!
//! Run: `cargo test -p buzz-test-client --test e2e_paid_runtime -- --ignored`
//!
//! The mock wallet is the test itself. Instead of paying a BOLT12 offer and
//! attesting the settled transaction, the test publishes the same agent-signed
//! kind 44210 deposit the wallet host would publish. Everything downstream is
//! the production code path: the relay's deposit envelope validation, the
//! payer's ledger read, the agent's mint loop (`ensure_open_reservations`),
//! and the relay trigger that binds one instruction to one reservation.

use std::collections::HashSet;
use std::time::Duration;

use buzz_acp::config::RespondTo;
use buzz_acp::paid_runtime::{ensure_open_reservations, PaidRuntimeTerms};
use buzz_acp::relay::RestClient;
use buzz_core::agent_runtime_payment::{
    RuntimeDeposit, RuntimePricing, RuntimeReservation, MILLIS_PER_MINUTE, VERSION,
};
use buzz_core::kind::{
    KIND_AGENT_RUNTIME_DEPOSIT, KIND_AGENT_RUNTIME_PRICING, KIND_AGENT_RUNTIME_RESERVATION,
    KIND_NIP29_GROUP_MEMBERS, KIND_STREAM_MESSAGE,
};
use buzz_test_client::BuzzTestClient;
use nostr::nips::nip44;
use nostr::{Alphabet, Event, EventBuilder, Filter, JsonUtil, Keys, Kind, SingleLetterTag, Tag};
use uuid::Uuid;

const RATE_SATS_PER_MINUTE: u64 = 20;
const PACK_MINUTES: u16 = 15;

fn relay_url() -> String {
    std::env::var("RELAY_URL").unwrap_or_else(|_| "ws://localhost:3000".to_string())
}

fn relay_http_url() -> String {
    relay_url()
        .replace("wss://", "https://")
        .replace("ws://", "http://")
        .trim_end_matches('/')
        .to_string()
}

fn rest_client(keys: &Keys) -> RestClient {
    RestClient {
        http: reqwest::Client::new(),
        base_url: relay_http_url(),
        keys: keys.clone(),
        auth_tag_json: None,
    }
}

async fn submit_accepted(rest: &RestClient, event: &Event, what: &str) {
    let value = rest
        .submit_event(event)
        .await
        .unwrap_or_else(|error| panic!("{what}: submit failed: {error}"));
    assert!(
        value["accepted"].as_bool().unwrap_or(false),
        "{what}: relay did not accept: {value}"
    );
}

async fn query_rows(rest: &RestClient, filter: Filter) -> Vec<serde_json::Value> {
    let value = rest.query(&[filter]).await.expect("bridge query");
    value.as_array().cloned().unwrap_or_default()
}

/// An unpublished signed event, used only to derive a syntactically valid
/// reference id for the mock zap and zap-intent tags.
fn mock_reference_id(keys: &Keys, label: &str) -> String {
    EventBuilder::new(Kind::Custom(1), label)
        .sign_with_keys(keys)
        .expect("sign mock reference")
        .id
        .to_hex()
}

fn deposit_content() -> String {
    let deposit = RuntimeDeposit {
        version: VERSION,
        pack_minutes: PACK_MINUTES,
        credit_ms: u64::from(PACK_MINUTES) * MILLIS_PER_MINUTE,
        price_per_minute_sats: RATE_SATS_PER_MINUTE,
        amount_sats: RATE_SATS_PER_MINUTE * u64::from(PACK_MINUTES),
    };
    deposit.validate().expect("mock deposit is valid");
    serde_json::to_string(&deposit).expect("serialize deposit")
}

#[tokio::test]
#[ignore]
async fn paid_runtime_prepaid_flow_with_mocked_wallet() {
    // The mint loop persists a pending-mint file; point it at a fresh
    // directory so reruns never replay a previous test's state.
    std::env::set_var(
        "BUZZ_ACP_RUNTIME_STATE_DIR",
        std::env::temp_dir().join(format!("buzz-paid-runtime-e2e-{}", Uuid::new_v4())),
    );

    let agent_keys = Keys::generate();
    let payer_keys = Keys::generate();
    let agent_rest = rest_client(&agent_keys);
    let payer_rest = rest_client(&payer_keys);
    let agent_hex = agent_keys.public_key().to_hex();
    let payer_hex = payer_keys.public_key().to_hex();
    let p_tag = SingleLetterTag::lowercase(Alphabet::P);
    let h_tag = SingleLetterTag::lowercase(Alphabet::H);
    let d_tag = SingleLetterTag::lowercase(Alphabet::D);

    // The agent opens a channel and adds the payer, so the pair may transact
    // there — the mint loop refuses scopes whose channel lacks either party.
    let channel_id = Uuid::new_v4().to_string();
    let create = EventBuilder::new(Kind::Custom(9007), "")
        .tags(vec![
            Tag::parse(["h", &channel_id]).unwrap(),
            Tag::parse(["name", &format!("paid-runtime-e2e-{channel_id}")]).unwrap(),
            Tag::parse(["channel_type", "stream"]).unwrap(),
            Tag::parse(["visibility", "open"]).unwrap(),
        ])
        .sign_with_keys(&agent_keys)
        .unwrap();
    submit_accepted(&agent_rest, &create, "create channel").await;
    let add_payer = EventBuilder::new(Kind::Custom(9000), "")
        .tags(vec![
            Tag::parse(["h", &channel_id]).unwrap(),
            Tag::parse(["p", &payer_hex]).unwrap(),
        ])
        .sign_with_keys(&agent_keys)
        .unwrap();
    submit_accepted(&agent_rest, &add_payer, "add payer to channel").await;

    // Wait until the relay's membership snapshot names the payer; the mint
    // loop reads it and silently skips the scope while it is stale.
    let mut member = false;
    for _ in 0..20 {
        let rows = query_rows(
            &agent_rest,
            Filter::new()
                .kind(Kind::Custom(KIND_NIP29_GROUP_MEMBERS as u16))
                .custom_tags(d_tag, [channel_id.as_str()]),
        )
        .await;
        member = rows.iter().any(|row| {
            row["tags"].as_array().is_some_and(|tags| {
                tags.iter().any(|tag| {
                    tag.as_array().is_some_and(|parts| {
                        parts.first().and_then(|part| part.as_str()) == Some("p")
                            && parts.get(1).and_then(|part| part.as_str())
                                == Some(payer_hex.as_str())
                    })
                })
            })
        });
        if member {
            break;
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    assert!(member, "membership snapshot never listed the payer");

    // Published terms: the agent's replaceable kind 10101 pricing.
    let pricing = RuntimePricing::enabled(RATE_SATS_PER_MINUTE).unwrap();
    let pricing_event = EventBuilder::new(
        Kind::Custom(KIND_AGENT_RUNTIME_PRICING as u16),
        serde_json::to_string(&pricing).unwrap(),
    )
    .sign_with_keys(&agent_keys)
    .unwrap();
    submit_accepted(&agent_rest, &pricing_event, "publish pricing").await;

    // Mock wallet attestation. This is the exact event the owner's desktop
    // publishes after verifying the settled BOLT12 payment: agent-signed
    // kind 44210, scoped by p and h, referencing pricing, zap, and intent.
    let deposit_tags = |include_pricing: bool| {
        let mut tags = vec![
            Tag::parse(["p", &payer_hex]).unwrap(),
            Tag::parse(["h", &channel_id]).unwrap(),
            Tag::parse(["zap", &mock_reference_id(&payer_keys, "mock zap receipt")]).unwrap(),
            Tag::parse([
                "zap_intent",
                &mock_reference_id(&payer_keys, "mock zap intent"),
            ])
            .unwrap(),
        ];
        if include_pricing {
            tags.push(Tag::parse(["pricing", &pricing_event.id.to_hex()]).unwrap());
        }
        tags
    };

    // Envelope regression: a deposit without its pricing reference must be
    // rejected, never stored as unattributed credit.
    let unpinned = EventBuilder::new(
        Kind::Custom(KIND_AGENT_RUNTIME_DEPOSIT as u16),
        deposit_content(),
    )
    .tags(deposit_tags(false))
    .sign_with_keys(&agent_keys)
    .unwrap();
    match agent_rest.submit_event(&unpinned).await {
        Ok(value) => assert!(
            !value["accepted"].as_bool().unwrap_or(false),
            "deposit without pricing tag must be rejected: {value}"
        ),
        Err(error) => assert!(
            error.to_string().contains("pricing"),
            "unexpected rejection for unpinned deposit: {error}"
        ),
    }

    let deposit = EventBuilder::new(
        Kind::Custom(KIND_AGENT_RUNTIME_DEPOSIT as u16),
        deposit_content(),
    )
    .tags(deposit_tags(true))
    .sign_with_keys(&agent_keys)
    .unwrap();
    submit_accepted(&agent_rest, &deposit, "publish deposit").await;

    // The payer sees its credit by reading its own ledger: authors + kinds +
    // #p. Ledger kinds are stored community-global (no channel column), so an
    // #h-scoped read finds nothing — the channel lives in the signed h tag,
    // and clients must post-filter on it. The empty result is asserted here
    // so a future "optimization" that adds #h back fails loudly instead of
    // silently zeroing every balance.
    let ledger_filter = Filter::new()
        .author(agent_keys.public_key())
        .kind(Kind::Custom(KIND_AGENT_RUNTIME_DEPOSIT as u16))
        .custom_tags(p_tag, [payer_hex.as_str()]);
    let visible = query_rows(&payer_rest, ledger_filter.clone()).await;
    assert_eq!(visible.len(), 1, "payer must see its deposit");
    let h_scoped = query_rows(
        &payer_rest,
        ledger_filter.custom_tags(h_tag, [channel_id.as_str()]),
    )
    .await;
    assert!(
        h_scoped.is_empty(),
        "ledger kinds are channel-less in storage; an #h filter must miss them"
    );

    // The agent's maintenance loop mints one open reservation from the
    // deposited credit. No request from the payer is involved.
    let terms = PaidRuntimeTerms {
        keys: agent_keys.clone(),
        respond_to: RespondTo::Anyone,
        respond_to_allowlist: HashSet::new(),
        priced: true,
    };
    let minted = ensure_open_reservations(&terms, &agent_rest)
        .await
        .expect("mint pass");
    assert_eq!(minted, 1, "one funded scope mints one reservation");
    let second_pass = ensure_open_reservations(&terms, &agent_rest)
        .await
        .expect("idempotent mint pass");
    assert_eq!(second_pass, 0, "a scope with an open lock mints nothing");

    // The payer discovers and decrypts the reservation from the same ledger.
    let reservations = query_rows(
        &payer_rest,
        Filter::new()
            .author(agent_keys.public_key())
            .kind(Kind::Custom(KIND_AGENT_RUNTIME_RESERVATION as u16))
            .custom_tags(p_tag, [payer_hex.as_str()]),
    )
    .await;
    assert_eq!(reservations.len(), 1, "payer must see its reservation");
    let reservation_event =
        Event::from_json(reservations[0].to_string()).expect("parse reservation event");
    let plaintext = nip44::decrypt(
        payer_keys.secret_key(),
        &agent_keys.public_key(),
        &reservation_event.content,
    )
    .expect("payer decrypts the reservation");
    let reservation: RuntimeReservation =
        serde_json::from_str(&plaintext).expect("parse reservation content");
    reservation.validate().expect("reservation is valid");
    assert_eq!(
        reservation.cap_ms,
        u64::from(PACK_MINUTES) * MILLIS_PER_MINUTE,
        "the first lock caps at the purchased pack"
    );
    let reservation_id = reservation_event.id.to_hex();

    // Invocation: the instruction carries the agent_runtime tag and the relay
    // trigger claims the reservation in the same insert transaction.
    let mut payer_ws = BuzzTestClient::connect(&relay_url(), &payer_keys)
        .await
        .expect("payer connects");
    let instruction = EventBuilder::new(
        Kind::Custom(KIND_STREAM_MESSAGE as u16),
        "please summarize this channel",
    )
    .tags(vec![
        Tag::parse(["h", &channel_id]).unwrap(),
        Tag::parse(["agent_runtime", &agent_hex, &reservation_id]).unwrap(),
    ])
    .sign_with_keys(&payer_keys)
    .unwrap();
    let ok = payer_ws
        .send_event(instruction.clone())
        .await
        .expect("send paid instruction");
    assert!(ok.accepted, "paid instruction rejected: {}", ok.message);

    // The same instruction is idempotent; a different one cannot steal the
    // claimed reservation.
    let replay = payer_ws
        .send_event(instruction)
        .await
        .expect("replay paid instruction");
    assert!(
        replay.accepted,
        "identical instruction must stay accepted: {}",
        replay.message
    );
    let thief = EventBuilder::new(Kind::Custom(KIND_STREAM_MESSAGE as u16), "run it again")
        .tags(vec![
            Tag::parse(["h", &channel_id]).unwrap(),
            Tag::parse(["agent_runtime", &agent_hex, &reservation_id]).unwrap(),
        ])
        .sign_with_keys(&payer_keys)
        .unwrap();
    let stolen = payer_ws
        .send_event(thief)
        .await
        .expect("send second instruction");
    assert!(
        !stolen.accepted,
        "a consumed reservation must reject a second instruction"
    );
    assert!(
        stolen.message.contains("unavailable or consumed"),
        "unexpected rejection message: {}",
        stolen.message
    );
    payer_ws.disconnect().await.expect("disconnect");
}

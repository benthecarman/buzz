//! E2E: paid Agent access through a BOLT12 zap.
//!
//! This test uses a placeholder payer proof. It exercises the production
//! relay and Agent admission paths, but it does not contact a Lightning node.
//!
//! Run: `cargo test -p buzz-test-client --test e2e_paid_runtime -- --ignored`

use std::{collections::HashSet, time::Duration};

use buzz_acp::{
    config::RespondTo,
    paid_runtime::{validate_instruction, PaidRuntimeTerms},
    relay::RestClient,
};
use buzz_core::{
    agent_runtime_payment::RuntimePricing,
    kind::{
        KIND_AGENT_RUNTIME_PRICING, KIND_BOLT12_OFFER, KIND_BOLT12_ZAP, KIND_BOLT12_ZAP_INTENT,
        KIND_NIP29_GROUP_MEMBERS, KIND_STREAM_MESSAGE, KIND_SYSTEM_MESSAGE,
    },
};
use buzz_test_client::BuzzTestClient;
use nostr::{Alphabet, Event, EventBuilder, Filter, JsonUtil, Keys, Kind, SingleLetterTag, Tag};
use uuid::Uuid;

const PRICE_SATS: u64 = 255;
const VALID_OFFER: &str =
    "lno1pgx9getnwss8vetrw3hhyuckyypwa3eyt44h6txtxquqh7lz5djge4afgfjn7k4rgrkuag0jsd5xvxg";

fn relay_url() -> String {
    std::env::var("RELAY_URL").unwrap_or_else(|_| "ws://localhost:3000".to_string())
}

fn rest_client(keys: &Keys) -> RestClient {
    RestClient {
        http: reqwest::Client::new(),
        base_url: relay_url()
            .replace("wss://", "https://")
            .replace("ws://", "http://")
            .trim_end_matches('/')
            .to_string(),
        keys: keys.clone(),
        auth_tag_json: None,
    }
}

async fn submit_accepted(rest: &RestClient, event: &Event, label: &str) {
    let value = rest
        .submit_event(event)
        .await
        .unwrap_or_else(|error| panic!("{label}: submit failed: {error}"));
    assert!(
        value["accepted"].as_bool().unwrap_or(false),
        "{label}: relay rejected the event: {value}"
    );
}

async fn wait_for_member(rest: &RestClient, channel_id: &str, payer_hex: &str) {
    let d_tag = SingleLetterTag::lowercase(Alphabet::D);
    for _ in 0..20 {
        let value = rest
            .query(&[Filter::new()
                .kind(Kind::Custom(KIND_NIP29_GROUP_MEMBERS as u16))
                .custom_tags(d_tag, [channel_id])])
            .await
            .expect("query membership");
        let found = value.as_array().is_some_and(|rows| {
            rows.iter().any(|row| {
                row["tags"].as_array().is_some_and(|tags| {
                    tags.iter().any(|tag| {
                        tag.as_array().is_some_and(|parts| {
                            parts.first().and_then(|part| part.as_str()) == Some("p")
                                && parts.get(1).and_then(|part| part.as_str()) == Some(payer_hex)
                        })
                    })
                })
            })
        });
        if found {
            return;
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    panic!("membership snapshot did not include the payer");
}

fn build_access_zap(
    payer: &Keys,
    agent: &Keys,
    channel_id: &str,
    pricing: &Event,
    offer: &Event,
) -> Event {
    let amount_msats = (PRICE_SATS * 1_000).to_string();
    let agent_hex = agent.public_key().to_hex();
    let pricing_id = pricing.id.to_hex();
    let intent = EventBuilder::new(Kind::Custom(KIND_BOLT12_ZAP_INTENT as u16), "Agent access")
        .tags([
            Tag::parse(["p", agent_hex.as_str()]).unwrap(),
            Tag::parse(["e", pricing_id.as_str()]).unwrap(),
            Tag::parse(["h", channel_id]).unwrap(),
            Tag::parse(["amount", amount_msats.as_str()]).unwrap(),
            Tag::parse(["offer_event", offer.as_json().as_str()]).unwrap(),
            Tag::parse(["zap_id", Uuid::new_v4().to_string().as_str()]).unwrap(),
        ])
        .sign_with_keys(payer)
        .unwrap();
    let mut tags = intent
        .tags
        .iter()
        .filter(|tag| tag.as_slice().first().map(String::as_str) != Some("zap_id"))
        .cloned()
        .collect::<Vec<_>>();
    tags.extend([
        Tag::parse(["description", intent.as_json().as_str()]).unwrap(),
        Tag::parse(["P", payer.public_key().to_hex().as_str()]).unwrap(),
        Tag::parse(["proof", "placeholder"]).unwrap(),
    ]);
    EventBuilder::new(Kind::Custom(KIND_BOLT12_ZAP as u16), intent.content)
        .tags(tags)
        .sign_with_keys(payer)
        .unwrap()
}

#[tokio::test]
#[ignore]
async fn paid_runtime_zap_grants_reusable_invocation_access() {
    let agent_keys = Keys::generate();
    let payer_keys = Keys::generate();
    let agent_rest = rest_client(&agent_keys);
    let payer_rest = rest_client(&payer_keys);
    let agent_hex = agent_keys.public_key().to_hex();
    let payer_hex = payer_keys.public_key().to_hex();
    let channel_id = Uuid::new_v4().to_string();

    let create = EventBuilder::new(Kind::Custom(9007), "")
        .tags([
            Tag::parse(["h", channel_id.as_str()]).unwrap(),
            Tag::parse(["name", format!("paid-agent-{channel_id}").as_str()]).unwrap(),
            Tag::parse(["channel_type", "stream"]).unwrap(),
            Tag::parse(["visibility", "open"]).unwrap(),
        ])
        .sign_with_keys(&agent_keys)
        .unwrap();
    submit_accepted(&agent_rest, &create, "create channel").await;
    let add_payer = EventBuilder::new(Kind::Custom(9000), "")
        .tags([
            Tag::parse(["h", channel_id.as_str()]).unwrap(),
            Tag::parse(["p", payer_hex.as_str()]).unwrap(),
        ])
        .sign_with_keys(&agent_keys)
        .unwrap();
    submit_accepted(&agent_rest, &add_payer, "add payer").await;
    wait_for_member(&agent_rest, &channel_id, &payer_hex).await;

    let offer = EventBuilder::new(Kind::Custom(KIND_BOLT12_OFFER as u16), "")
        .tag(Tag::parse(["offer", VALID_OFFER]).unwrap())
        .sign_with_keys(&agent_keys)
        .unwrap();
    submit_accepted(&agent_rest, &offer, "publish offer").await;
    let pricing = EventBuilder::new(
        Kind::Custom(KIND_AGENT_RUNTIME_PRICING as u16),
        serde_json::to_string(&RuntimePricing::enabled(PRICE_SATS).unwrap()).unwrap(),
    )
    .sign_with_keys(&agent_keys)
    .unwrap();
    submit_accepted(&agent_rest, &pricing, "publish pricing").await;

    let zap = build_access_zap(&payer_keys, &agent_keys, &channel_id, &pricing, &offer);
    submit_accepted(&payer_rest, &zap, "publish access zap").await;

    let h_tag = SingleLetterTag::lowercase(Alphabet::H);
    let channel_activity = agent_rest
        .query(&[Filter::new()
            .kinds([
                Kind::Custom(KIND_BOLT12_ZAP as u16),
                Kind::Custom(KIND_SYSTEM_MESSAGE as u16),
            ])
            .custom_tags(h_tag, [channel_id.as_str()])])
        .await
        .expect("query channel payment activity");
    let rows = channel_activity.as_array().expect("activity rows");
    assert!(
        rows.iter().any(|row| row["id"] == zap.id.to_hex()),
        "the signed zap must be the channel activity event"
    );
    assert!(
        rows.iter().all(|row| {
            row["kind"] != KIND_SYSTEM_MESSAGE
                || !row["content"]
                    .as_str()
                    .is_some_and(|content| content.contains("agent_runtime_purchased"))
        }),
        "the relay must not create a duplicate payment announcement"
    );

    let terms = PaidRuntimeTerms {
        keys: agent_keys.clone(),
        respond_to: RespondTo::Anyone,
        respond_to_allowlist: HashSet::new(),
        priced: true,
    };
    let mut payer_ws = BuzzTestClient::connect(&relay_url(), &payer_keys)
        .await
        .expect("connect payer");
    // The access contract has no invocation-count limit. This count is one
    // above the retired limiter, so this test prevents that limit from
    // returning under another name.
    for index in 0..31 {
        let instruction = EventBuilder::new(
            Kind::Custom(KIND_STREAM_MESSAGE as u16),
            format!("paid invocation {index}"),
        )
        .tags([
            Tag::parse(["h", channel_id.as_str()]).unwrap(),
            Tag::parse([
                "agent_runtime",
                agent_hex.as_str(),
                zap.id.to_hex().as_str(),
            ])
            .unwrap(),
        ])
        .sign_with_keys(&payer_keys)
        .unwrap();
        let response = payer_ws
            .send_event(instruction.clone())
            .await
            .expect("send paid instruction");
        assert!(
            response.accepted,
            "instruction rejected: {}",
            response.message
        );
        validate_instruction(&terms, &agent_rest, &instruction, &channel_id)
            .await
            .expect("Agent accepts the paid instruction");
    }

    let other_payer = Keys::generate();
    let wrong_author = EventBuilder::new(Kind::Custom(KIND_STREAM_MESSAGE as u16), "wrong payer")
        .tags([
            Tag::parse(["h", channel_id.as_str()]).unwrap(),
            Tag::parse([
                "agent_runtime",
                agent_hex.as_str(),
                zap.id.to_hex().as_str(),
            ])
            .unwrap(),
        ])
        .sign_with_keys(&other_payer)
        .unwrap();
    assert!(
        validate_instruction(&terms, &agent_rest, &wrong_author, &channel_id)
            .await
            .is_err(),
        "Agent accepted a zap that belongs to a different payer"
    );

    let wrong_channel_id = Uuid::new_v4().to_string();
    let wrong_channel =
        EventBuilder::new(Kind::Custom(KIND_STREAM_MESSAGE as u16), "wrong channel")
            .tags([
                Tag::parse(["h", wrong_channel_id.as_str()]).unwrap(),
                Tag::parse([
                    "agent_runtime",
                    agent_hex.as_str(),
                    zap.id.to_hex().as_str(),
                ])
                .unwrap(),
            ])
            .sign_with_keys(&payer_keys)
            .unwrap();
    assert!(
        validate_instruction(&terms, &agent_rest, &wrong_channel, &channel_id)
            .await
            .is_err(),
        "Agent accepted an instruction from a different channel"
    );

    payer_ws.disconnect().await.expect("disconnect payer");
}

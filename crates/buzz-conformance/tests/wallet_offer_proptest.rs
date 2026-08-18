use std::collections::BTreeSet;

use buzz_conformance::wallet_offer::{
    apply_offer_action, check_offer_trace, OfferAbstractState, OfferCheckerConfig, OfferId,
    OfferIdentityId, OfferRelayId, OfferTraceAction, OfferTraceStep,
};
use proptest::prelude::*;

fn identity() -> OfferIdentityId {
    OfferIdentityId("generated-agent".to_string())
}

fn offer() -> OfferId {
    OfferId("generated-offer".to_string())
}

fn relay(index: usize) -> OfferRelayId {
    OfferRelayId(format!("relay-{index}"))
}

fn append(
    trace: &mut Vec<OfferTraceStep>,
    state: &mut OfferAbstractState,
    action: OfferTraceAction,
) {
    let before = state.clone();
    *state = apply_offer_action(&before, &action).expect("generated legal action");
    trace.push(OfferTraceStep::new(action, before, state.clone()));
}

proptest! {
    #[test]
    fn every_target_outcome_sequence_is_accepted(
        outcomes in prop::collection::vec(any::<bool>(), 1..8)
    ) {
        let targets = (0..outcomes.len()).map(relay).collect::<Vec<_>>();
        let mut state = OfferAbstractState::default();
        let mut trace = Vec::new();
        append(
            &mut trace,
            &mut state,
            OfferTraceAction::BeginAnnouncement {
                identity: identity(),
                offer: offer(),
                target_relays: targets.clone(),
                event_kind: 10_058,
                author_matches: true,
                issuer_matches_wallet_owner: true,
            },
        );
        for (target, accepted) in targets.into_iter().zip(outcomes) {
            append(
                &mut trace,
                &mut state,
                OfferTraceAction::RelayResult {
                    identity: identity(),
                    relay: target,
                    accepted,
                },
            );
        }
        append(
            &mut trace,
            &mut state,
            OfferTraceAction::FinishAnnouncement { identity: identity() },
        );
        prop_assert!(check_offer_trace(
            &trace,
            &OfferCheckerConfig::default().require("finish_announcement")
        ).is_ok());
    }

    #[test]
    fn finishing_with_any_missing_target_is_rejected(target_count in 1usize..8) {
        let targets = (0..target_count).map(relay).collect::<Vec<_>>();
        let mut state = OfferAbstractState::default();
        state = apply_offer_action(
            &state,
            &OfferTraceAction::BeginWithdrawal {
                identity: identity(),
                target_relays: targets.clone(),
                event_kind: 10_058,
                author_matches: true,
            },
        ).unwrap();
        for target in targets.iter().take(target_count - 1) {
            state = apply_offer_action(
                &state,
                &OfferTraceAction::RelayResult {
                    identity: identity(),
                    relay: target.clone(),
                    accepted: true,
                },
            ).unwrap();
        }
        let early_finish = apply_offer_action(
            &state,
            &OfferTraceAction::FinishWithdrawal { identity: identity() }
        );
        prop_assert!(early_finish.is_err());
    }
}

#[test]
fn duplicate_target_list_is_rejected_before_set_projection() {
    let action = OfferTraceAction::BeginAnnouncement {
        identity: identity(),
        offer: offer(),
        target_relays: vec![relay(0), relay(0)],
        event_kind: 10_058,
        author_matches: true,
        issuer_matches_wallet_owner: true,
    };
    assert!(apply_offer_action(&OfferAbstractState::default(), &action).is_err());
}

#[test]
fn target_ids_are_distinct_in_the_generator() {
    let relays = (0..8).map(relay).collect::<BTreeSet<_>>();
    assert_eq!(relays.len(), 8);
}

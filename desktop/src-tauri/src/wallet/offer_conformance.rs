//! Secret-free runtime trace emitter for kind `10058` offer fan-out.

use std::{
    collections::BTreeSet,
    fs::OpenOptions,
    io::Write,
    sync::{Mutex, OnceLock},
};

use buzz_conformance_pkg::wallet_offer::{
    OfferAbstractState, OfferId, OfferIdentityId, OfferIdentityState, OfferPhase, OfferRelayId,
    OfferTraceAction, OfferTraceStep,
};
use nostr::Event;
use sha2_10::{Digest, Sha256};

#[derive(Clone, Copy)]
enum PublicationKind {
    Announcement,
    Withdrawal,
}

/// One in-flight projection at the signed-event publication seam.
pub struct OfferPublicationTrace {
    identity: OfferIdentityId,
    kind: PublicationKind,
    finished: bool,
}

impl OfferPublicationTrace {
    /// Begin tracing the exact signed event and relay targets that production
    /// is about to publish.
    pub fn start(
        event: &Event,
        keys: &nostr::Keys,
        offer_issuer: Option<&nostr::Keys>,
        wallet_owner: &nostr::Keys,
        relay_urls: &[String],
    ) -> Self {
        let identity = opaque_identity(&keys.public_key().to_hex());
        let target_relays = relay_urls
            .iter()
            .map(|relay| opaque_relay(relay))
            .collect::<Vec<_>>();
        let event_kind = event.kind.as_u16() as u32;
        let author_matches = event.pubkey == keys.public_key();
        let offer = event.tags.iter().find_map(|tag| {
            let parts = tag.as_slice();
            (parts.first().map(String::as_str) == Some("offer"))
                .then(|| parts.get(1))
                .flatten()
                .map(|value| opaque_offer(value))
        });
        let (kind, action) = match offer {
            Some(offer) => (
                PublicationKind::Announcement,
                OfferTraceAction::BeginAnnouncement {
                    identity: identity.clone(),
                    offer,
                    target_relays,
                    event_kind,
                    author_matches,
                    issuer_matches_wallet_owner: offer_issuer
                        .is_some_and(|issuer| issuer.public_key() == wallet_owner.public_key()),
                },
            ),
            None => (
                PublicationKind::Withdrawal,
                OfferTraceAction::BeginWithdrawal {
                    identity: identity.clone(),
                    target_relays,
                    event_kind,
                    author_matches,
                },
            ),
        };
        record(action);
        Self {
            identity,
            kind,
            finished: false,
        }
    }

    /// Record the terminal result for one target relay.
    pub fn relay_result(&self, relay_url: &str, accepted: bool) {
        record(OfferTraceAction::RelayResult {
            identity: self.identity.clone(),
            relay: opaque_relay(relay_url),
            accepted,
        });
    }

    /// Complete the operation after every configured relay produced a result.
    pub fn finish(mut self) {
        let action = match self.kind {
            PublicationKind::Announcement => OfferTraceAction::FinishAnnouncement {
                identity: self.identity.clone(),
            },
            PublicationKind::Withdrawal => OfferTraceAction::FinishWithdrawal {
                identity: self.identity.clone(),
            },
        };
        record(action);
        self.finished = true;
    }

    /// Abort an operation after a hard active-relay failure.
    pub fn abort(mut self) {
        record(OfferTraceAction::Abort {
            identity: self.identity.clone(),
        });
        self.finished = true;
    }
}

impl Drop for OfferPublicationTrace {
    fn drop(&mut self) {
        if !self.finished {
            record(OfferTraceAction::ImplBug {
                identity: self.identity.clone(),
            });
        }
    }
}

fn record(action: OfferTraceAction) {
    let state = OFFER_STATE.get_or_init(|| Mutex::new(OfferAbstractState::default()));
    let Ok(mut state) = state.lock() else {
        tracing::warn!("wallet offer trace state lock is poisoned");
        return;
    };
    let before = state.clone();
    let (emitted_action, after) = match apply_projection(&before, &action) {
        Ok(after) => (action, after),
        Err(()) => {
            let identity = action_identity(&action).clone();
            (OfferTraceAction::ImplBug { identity }, before.clone())
        }
    };
    *state = after.clone();
    drop(state);
    emit(OfferTraceStep::new(emitted_action, before, after));
}

/// Production-side projection reducer. The independent checker has its own
/// translation of the TLA+ `Next` relation.
fn apply_projection(
    before: &OfferAbstractState,
    action: &OfferTraceAction,
) -> Result<OfferAbstractState, ()> {
    let mut after = before.clone();
    let identity = action_identity(action).clone();
    let current = before
        .identities
        .get(&identity)
        .cloned()
        .unwrap_or_else(OfferIdentityState::idle);
    match action {
        OfferTraceAction::BeginAnnouncement {
            offer,
            target_relays,
            event_kind,
            author_matches,
            issuer_matches_wallet_owner,
            ..
        } => {
            validate_begin(&current, target_relays, *event_kind, *author_matches)?;
            if !issuer_matches_wallet_owner {
                return Err(());
            }
            if before.identities.iter().any(|(other, state)| {
                other != &identity
                    && (state.active_offer.as_ref() == Some(offer)
                        || state.pending_offer.as_ref() == Some(offer))
            }) {
                return Err(());
            }
            let mut next = current;
            next.phase = OfferPhase::Announcing;
            next.pending_offer = Some(offer.clone());
            next.target_relays = target_relays.iter().cloned().collect();
            next.attempted_relays.clear();
            next.accepted_relays.clear();
            after.identities.insert(identity, next);
        }
        OfferTraceAction::BeginWithdrawal {
            target_relays,
            event_kind,
            author_matches,
            ..
        } => {
            validate_begin(&current, target_relays, *event_kind, *author_matches)?;
            let mut next = current;
            next.phase = OfferPhase::Withdrawing;
            next.pending_offer = None;
            next.target_relays = target_relays.iter().cloned().collect();
            next.attempted_relays.clear();
            next.accepted_relays.clear();
            after.identities.insert(identity, next);
        }
        OfferTraceAction::RelayResult {
            relay, accepted, ..
        } => {
            if current.phase == OfferPhase::Idle
                || !current.target_relays.contains(relay)
                || current.attempted_relays.contains(relay)
            {
                return Err(());
            }
            let mut next = current;
            next.attempted_relays.insert(relay.clone());
            if *accepted {
                next.accepted_relays.insert(relay.clone());
            }
            after.identities.insert(identity, next);
        }
        OfferTraceAction::FinishAnnouncement { .. } => {
            if current.phase != OfferPhase::Announcing
                || current.attempted_relays != current.target_relays
            {
                return Err(());
            }
            let mut next = current;
            next.phase = OfferPhase::Idle;
            next.active_offer = next.pending_offer.take();
            clear_operation(&mut next);
            after.identities.insert(identity, next);
        }
        OfferTraceAction::FinishWithdrawal { .. } => {
            if current.phase != OfferPhase::Withdrawing
                || current.attempted_relays != current.target_relays
            {
                return Err(());
            }
            let mut next = current;
            next.phase = OfferPhase::Idle;
            next.active_offer = None;
            clear_operation(&mut next);
            after.identities.insert(identity, next);
        }
        OfferTraceAction::Abort { .. } => {
            if current.phase == OfferPhase::Idle {
                return Err(());
            }
            let mut next = current;
            next.phase = OfferPhase::Idle;
            clear_operation(&mut next);
            after.identities.insert(identity, next);
        }
        OfferTraceAction::ImplBug { .. } => return Err(()),
    }
    Ok(after)
}

fn validate_begin(
    current: &OfferIdentityState,
    targets: &[OfferRelayId],
    event_kind: u32,
    author_matches: bool,
) -> Result<(), ()> {
    if current.phase != OfferPhase::Idle
        || targets.is_empty()
        || targets.iter().collect::<BTreeSet<_>>().len() != targets.len()
        || event_kind != 10_058
        || !author_matches
    {
        Err(())
    } else {
        Ok(())
    }
}

fn action_identity(action: &OfferTraceAction) -> &OfferIdentityId {
    match action {
        OfferTraceAction::BeginAnnouncement { identity, .. }
        | OfferTraceAction::BeginWithdrawal { identity, .. }
        | OfferTraceAction::RelayResult { identity, .. }
        | OfferTraceAction::FinishAnnouncement { identity }
        | OfferTraceAction::FinishWithdrawal { identity }
        | OfferTraceAction::Abort { identity }
        | OfferTraceAction::ImplBug { identity } => identity,
    }
}

fn clear_operation(state: &mut OfferIdentityState) {
    state.pending_offer = None;
    state.target_relays.clear();
    state.attempted_relays.clear();
    state.accepted_relays.clear();
}

fn opaque_identity(pubkey: &str) -> OfferIdentityId {
    OfferIdentityId(opaque(b"buzz-wallet-offer-identity-v1\0", pubkey))
}

fn opaque_offer(offer: &str) -> OfferId {
    OfferId(opaque(b"buzz-wallet-offer-v1\0", offer))
}

fn opaque_relay(relay: &str) -> OfferRelayId {
    OfferRelayId(opaque(b"buzz-wallet-offer-relay-v1\0", relay))
}

fn opaque(domain: &[u8], value: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(domain);
    hasher.update(value.as_bytes());
    hex::encode(&hasher.finalize()[..16])
}

fn emit(step: OfferTraceStep) {
    #[cfg(test)]
    if let Some(trace) = TEST_TRACE.get() {
        if let Ok(mut trace) = trace.lock() {
            trace.push(step.clone());
        }
    }

    let Some(path) = std::env::var_os("BUZZ_WALLET_OFFER_TRACE_PATH") else {
        return;
    };
    let sink = TRACE_SINK.get_or_init(|| {
        OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .map(Mutex::new)
    });
    let Ok(sink) = sink else {
        tracing::warn!("open BUZZ_WALLET_OFFER_TRACE_PATH failed");
        return;
    };
    let Ok(mut sink) = sink.lock() else {
        tracing::warn!("wallet offer trace sink lock is poisoned");
        return;
    };
    let Ok(mut line) = serde_json::to_vec(&step) else {
        tracing::warn!("serialize wallet offer trace step failed");
        return;
    };
    line.push(b'\n');
    if let Err(error) = sink.write_all(&line).and_then(|()| sink.flush()) {
        tracing::warn!(%error, "write wallet offer trace step failed");
    }
}

static OFFER_STATE: OnceLock<Mutex<OfferAbstractState>> = OnceLock::new();
static TRACE_SINK: OnceLock<std::io::Result<Mutex<std::fs::File>>> = OnceLock::new();

#[cfg(test)]
static TEST_TRACE: OnceLock<Mutex<Vec<OfferTraceStep>>> = OnceLock::new();

#[cfg(test)]
static TEST_LOCK: Mutex<()> = Mutex::new(());

#[cfg(test)]
fn reset_test_trace() {
    *OFFER_STATE
        .get_or_init(|| Mutex::new(OfferAbstractState::default()))
        .lock()
        .unwrap() = OfferAbstractState::default();
    TEST_TRACE
        .get_or_init(|| Mutex::new(Vec::new()))
        .lock()
        .unwrap()
        .clear();
}

#[cfg(test)]
fn take_test_trace() -> Vec<OfferTraceStep> {
    std::mem::take(
        &mut *TEST_TRACE
            .get_or_init(|| Mutex::new(Vec::new()))
            .lock()
            .unwrap(),
    )
}

#[cfg(test)]
mod tests {
    use buzz_conformance_pkg::wallet_offer::{check_offer_trace, OfferCheckerConfig};
    use nostr::{EventBuilder, Kind};

    use super::*;

    #[test]
    fn signed_announcement_projection_replays_against_the_independent_checker() {
        let _guard = TEST_LOCK.lock().unwrap();
        reset_test_trace();
        let keys = nostr::Keys::generate();
        let event = EventBuilder::new(Kind::Custom(10_058), "")
            .tag(nostr::Tag::parse(["offer", crate::wallet::VALID_OFFER]).unwrap())
            .sign_with_keys(&keys)
            .unwrap();
        let trace = OfferPublicationTrace::start(
            &event,
            &keys,
            Some(&keys),
            &keys,
            &[
                "https://one.example".to_string(),
                "https://two.example".to_string(),
            ],
        );
        trace.relay_result("https://one.example", true);
        trace.relay_result("https://two.example", false);
        trace.finish();

        let withdrawal = EventBuilder::new(Kind::Custom(10_058), "")
            .sign_with_keys(&keys)
            .unwrap();
        let trace = OfferPublicationTrace::start(
            &withdrawal,
            &keys,
            None,
            &keys,
            &[
                "https://one.example".to_string(),
                "https://two.example".to_string(),
            ],
        );
        trace.relay_result("https://one.example", true);
        trace.relay_result("https://two.example", true);
        trace.finish();

        check_offer_trace(
            &take_test_trace(),
            &OfferCheckerConfig::default()
                .require("begin_announcement")
                .require("begin_withdrawal")
                .require("relay_result")
                .require("finish_announcement")
                .require("finish_withdrawal"),
        )
        .unwrap();
    }

    #[test]
    fn mismatched_signer_emits_a_coverage_breach() {
        let _guard = TEST_LOCK.lock().unwrap();
        reset_test_trace();
        let author = nostr::Keys::generate();
        let publisher = nostr::Keys::generate();
        let event = EventBuilder::new(Kind::Custom(10_058), "")
            .sign_with_keys(&author)
            .unwrap();
        let trace = OfferPublicationTrace::start(
            &event,
            &publisher,
            None,
            &publisher,
            &["https://one.example".to_string()],
        );
        trace.abort();
        assert!(check_offer_trace(&take_test_trace(), &OfferCheckerConfig::default()).is_err());
    }

    #[test]
    fn agent_offer_from_agent_wallet_emits_a_coverage_breach() {
        let _guard = TEST_LOCK.lock().unwrap();
        reset_test_trace();
        let owner = nostr::Keys::generate();
        let agent = nostr::Keys::generate();
        let event = EventBuilder::new(Kind::Custom(10_058), "")
            .tag(nostr::Tag::parse(["offer", crate::wallet::VALID_OFFER]).unwrap())
            .sign_with_keys(&agent)
            .unwrap();
        let trace = OfferPublicationTrace::start(
            &event,
            &agent,
            Some(&agent),
            &owner,
            &["https://one.example".to_string()],
        );
        trace.abort();
        assert!(check_offer_trace(&take_test_trace(), &OfferCheckerConfig::default()).is_err());
    }
}

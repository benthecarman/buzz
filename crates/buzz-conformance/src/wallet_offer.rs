//! Independent runtime checker for `docs/spec/WalletOfferLifecycle.tla`.
//!
//! The desktop wallet emits this schema at the signed kind `10058` fan-out
//! seam. This module depends on no production wallet or relay code.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

/// Current offer-publication trace schema version.
pub const OFFER_TRACE_SCHEMA_VERSION: u32 = 2;

macro_rules! opaque_id {
    ($name:ident, $doc:literal) => {
        #[doc = $doc]
        #[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(pub String);
    };
}

opaque_id!(OfferIdentityId, "Secret-free identity label.");
opaque_id!(OfferId, "Secret-free BOLT12 offer label.");
opaque_id!(OfferRelayId, "Secret-free relay label.");

/// Publication phase for one identity.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OfferPhase {
    /// No publication is in progress.
    Idle,
    /// An offer announcement is being fanned out.
    Announcing,
    /// An empty replacement announcement is being fanned out.
    Withdrawing,
}

/// Projection of one identity's offer-publication state.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OfferIdentityState {
    /// Current publication phase.
    pub phase: OfferPhase,
    /// Most recently completed active offer.
    pub active_offer: Option<OfferId>,
    /// Offer being announced, if any.
    pub pending_offer: Option<OfferId>,
    /// Exact relay targets for the current operation.
    pub target_relays: BTreeSet<OfferRelayId>,
    /// Targets for which the implementation observed a result.
    pub attempted_relays: BTreeSet<OfferRelayId>,
    /// Attempted targets that accepted the event.
    pub accepted_relays: BTreeSet<OfferRelayId>,
}

impl OfferIdentityState {
    /// Initial state for an identity not yet observed by the checker.
    pub fn idle() -> Self {
        Self {
            phase: OfferPhase::Idle,
            active_offer: None,
            pending_offer: None,
            target_relays: BTreeSet::new(),
            attempted_relays: BTreeSet::new(),
            accepted_relays: BTreeSet::new(),
        }
    }
}

/// Secret-free global projection used to enforce cross-identity uniqueness.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct OfferAbstractState {
    /// Every identity observed in this trace.
    pub identities: BTreeMap<OfferIdentityId, OfferIdentityState>,
}

/// Critical decisions at the kind `10058` publication seam.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum OfferTraceAction {
    /// Start publishing an offer announcement.
    BeginAnnouncement {
        /// Event author and wallet identity.
        identity: OfferIdentityId,
        /// Opaque canonical offer identifier.
        offer: OfferId,
        /// Exact relay target list before set projection.
        target_relays: Vec<OfferRelayId>,
        /// Actual event kind observed at the seam.
        event_kind: u32,
        /// Whether the signed event author equals the publishing identity.
        author_matches: bool,
        /// Whether the offer issuer is the active user's wallet.
        issuer_matches_wallet_owner: bool,
    },
    /// Start publishing an empty replacement announcement.
    BeginWithdrawal {
        /// Event author and wallet identity.
        identity: OfferIdentityId,
        /// Exact relay target list before set projection.
        target_relays: Vec<OfferRelayId>,
        /// Actual event kind observed at the seam.
        event_kind: u32,
        /// Whether the signed event author equals the publishing identity.
        author_matches: bool,
    },
    /// Record one target relay's accept or reject result.
    RelayResult {
        /// Publishing identity.
        identity: OfferIdentityId,
        /// Relay for which a result was observed.
        relay: OfferRelayId,
        /// Whether that relay accepted the signed event.
        accepted: bool,
    },
    /// Complete an announcement after every target was attempted.
    FinishAnnouncement {
        /// Publishing identity.
        identity: OfferIdentityId,
    },
    /// Complete a withdrawal after every target was attempted.
    FinishWithdrawal {
        /// Publishing identity.
        identity: OfferIdentityId,
    },
    /// Abort after a hard publication error.
    Abort {
        /// Publishing identity.
        identity: OfferIdentityId,
    },
    /// Runtime witness that the emitter could not project a legal action.
    ImplBug {
        /// Identity whose operation could not be projected.
        identity: OfferIdentityId,
    },
}

impl OfferTraceAction {
    /// Stable action name used by coverage expectations.
    pub const fn kind(&self) -> &'static str {
        match self {
            Self::BeginAnnouncement { .. } => "begin_announcement",
            Self::BeginWithdrawal { .. } => "begin_withdrawal",
            Self::RelayResult { .. } => "relay_result",
            Self::FinishAnnouncement { .. } => "finish_announcement",
            Self::FinishWithdrawal { .. } => "finish_withdrawal",
            Self::Abort { .. } => "abort",
            Self::ImplBug { .. } => "impl_bug",
        }
    }
}

/// One JSONL offer-publication trace step.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OfferTraceStep {
    /// Schema version.
    pub schema_version: u32,
    /// Critical implementation decision.
    pub action: OfferTraceAction,
    /// Projection immediately before the decision.
    pub state_before: OfferAbstractState,
    /// Projection immediately after the decision.
    pub state_after: OfferAbstractState,
}

impl OfferTraceStep {
    /// Construct a step at the current schema version.
    pub fn new(
        action: OfferTraceAction,
        state_before: OfferAbstractState,
        state_after: OfferAbstractState,
    ) -> Self {
        Self {
            schema_version: OFFER_TRACE_SCHEMA_VERSION,
            action,
            state_before,
            state_after,
        }
    }
}

/// Scenario-specific coverage requirements.
#[derive(Debug, Clone, Default)]
pub struct OfferCheckerConfig {
    /// Critical action names the scenario must exercise.
    pub required_critical_actions: BTreeSet<String>,
}

impl OfferCheckerConfig {
    /// Require one action kind to occur.
    pub fn require(mut self, kind: &str) -> Self {
        self.required_critical_actions.insert(kind.to_string());
        self
    }
}

/// Independent replay failure.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum OfferCheckError {
    /// The action is forbidden from the checker-computed prior state.
    #[error("illegal offer transition at step {step}: {reason}")]
    IllegalTransition {
        /// Zero-based trace index.
        step: usize,
        /// Rule that rejected the transition.
        reason: String,
    },
    /// The implementation projection differs from checker-computed state.
    #[error("offer state mismatch at step {step}")]
    StateMismatch {
        /// Zero-based trace index.
        step: usize,
        /// Independently computed state.
        expected: OfferAbstractState,
        /// Implementation-emitted state.
        observed: OfferAbstractState,
    },
    /// A critical action was missing, unknown, malformed, or uncovered.
    #[error("offer trace coverage breach: {reason}")]
    CoverageBreach {
        /// Coverage failure detail.
        reason: String,
    },
}

/// Replay implementation steps against the spec transition relation.
pub fn check_offer_trace(
    trace: &[OfferTraceStep],
    config: &OfferCheckerConfig,
) -> Result<(), OfferCheckError> {
    if trace.is_empty() {
        return Err(OfferCheckError::CoverageBreach {
            reason: "trace is empty".to_string(),
        });
    }
    let mut model = OfferAbstractState::default();
    let mut seen = BTreeSet::new();
    for (index, step) in trace.iter().enumerate() {
        if step.schema_version != OFFER_TRACE_SCHEMA_VERSION {
            return Err(OfferCheckError::CoverageBreach {
                reason: format!("step {index} uses an unsupported schema version"),
            });
        }
        if matches!(step.action, OfferTraceAction::ImplBug { .. }) {
            return Err(OfferCheckError::CoverageBreach {
                reason: format!("implementation reported an uncovered path at step {index}"),
            });
        }
        if step.state_before != model {
            return Err(OfferCheckError::StateMismatch {
                step: index,
                expected: model,
                observed: step.state_before.clone(),
            });
        }
        let expected = apply_offer_action(&model, &step.action).map_err(|reason| {
            OfferCheckError::IllegalTransition {
                step: index,
                reason,
            }
        })?;
        if step.state_after != expected {
            return Err(OfferCheckError::StateMismatch {
                step: index,
                expected,
                observed: step.state_after.clone(),
            });
        }
        model = step.state_after.clone();
        seen.insert(step.action.kind().to_string());
    }
    let missing = config
        .required_critical_actions
        .iter()
        .filter(|kind| !seen.contains(*kind))
        .cloned()
        .collect::<Vec<_>>();
    if missing.is_empty() {
        Ok(())
    } else {
        Err(OfferCheckError::CoverageBreach {
            reason: format!("missing required critical actions: {missing:?}"),
        })
    }
}

/// Parse JSONL and fail closed on unknown or malformed actions.
pub fn check_offer_jsonl(jsonl: &str, config: &OfferCheckerConfig) -> Result<(), OfferCheckError> {
    let mut trace = Vec::new();
    for (index, line) in jsonl.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        trace.push(serde_json::from_str(line).map_err(|error| {
            OfferCheckError::CoverageBreach {
                reason: format!("unknown or malformed action on line {}: {error}", index + 1),
            }
        })?);
    }
    check_offer_trace(&trace, config)
}

/// Independent implementation of `WalletOfferLifecycle.tla`'s `Next` relation.
pub fn apply_offer_action(
    before: &OfferAbstractState,
    action: &OfferTraceAction,
) -> Result<OfferAbstractState, String> {
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
            require_begin(&current, target_relays, *event_kind, *author_matches)?;
            if !issuer_matches_wallet_owner {
                return Err("offer was not issued by the user's wallet".to_string());
            }
            if before.identities.iter().any(|(other, state)| {
                other != &identity
                    && (state.active_offer.as_ref() == Some(offer)
                        || state.pending_offer.as_ref() == Some(offer))
            }) {
                return Err("offer is already assigned to another identity".to_string());
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
            require_begin(&current, target_relays, *event_kind, *author_matches)?;
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
            if current.phase == OfferPhase::Idle {
                return Err("relay result observed without a publication".to_string());
            }
            if !current.target_relays.contains(relay) {
                return Err("relay result is outside the target set".to_string());
            }
            if current.attempted_relays.contains(relay) {
                return Err("relay was attempted more than once".to_string());
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
                return Err("announcement finished before all targets were attempted".to_string());
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
                return Err("withdrawal finished before all targets were attempted".to_string());
            }
            let mut next = current;
            next.phase = OfferPhase::Idle;
            next.active_offer = None;
            clear_operation(&mut next);
            after.identities.insert(identity, next);
        }
        OfferTraceAction::Abort { .. } => {
            if current.phase == OfferPhase::Idle {
                return Err("idle publication cannot abort".to_string());
            }
            let mut next = current;
            next.phase = OfferPhase::Idle;
            next.pending_offer = None;
            clear_operation(&mut next);
            after.identities.insert(identity, next);
        }
        OfferTraceAction::ImplBug { .. } => {
            return Err("impl_bug is a coverage breach".to_string());
        }
    }
    Ok(after)
}

fn require_begin(
    current: &OfferIdentityState,
    targets: &[OfferRelayId],
    event_kind: u32,
    author_matches: bool,
) -> Result<(), String> {
    if current.phase != OfferPhase::Idle {
        return Err("publication began while another was active".to_string());
    }
    if targets.is_empty() {
        return Err("publication target set is empty".to_string());
    }
    if targets.iter().collect::<BTreeSet<_>>().len() != targets.len() {
        return Err("publication target list contains duplicates".to_string());
    }
    if event_kind != 10_058 {
        return Err(format!("publication used event kind {event_kind}"));
    }
    if !author_matches {
        return Err("event author does not match the wallet identity".to_string());
    }
    Ok(())
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

#[cfg(test)]
mod tests {
    use super::*;

    fn id(value: &str) -> OfferIdentityId {
        OfferIdentityId(value.to_string())
    }
    fn offer(value: &str) -> OfferId {
        OfferId(value.to_string())
    }
    fn relay(value: &str) -> OfferRelayId {
        OfferRelayId(value.to_string())
    }
    fn append(
        trace: &mut Vec<OfferTraceStep>,
        state: &mut OfferAbstractState,
        action: OfferTraceAction,
    ) {
        let before = state.clone();
        *state = apply_offer_action(&before, &action).unwrap();
        trace.push(OfferTraceStep::new(action, before, state.clone()));
    }

    #[test]
    fn complete_multi_identity_fanout_is_accepted() {
        let mut state = OfferAbstractState::default();
        let mut trace = Vec::new();
        for (identity, offer_id) in [("owner", "offer-owner"), ("agent", "offer-agent")] {
            append(
                &mut trace,
                &mut state,
                OfferTraceAction::BeginAnnouncement {
                    identity: id(identity),
                    offer: offer(offer_id),
                    target_relays: vec![relay("a"), relay("b")],
                    event_kind: 10_058,
                    author_matches: true,
                    issuer_matches_wallet_owner: true,
                },
            );
            for target in ["a", "b"] {
                append(
                    &mut trace,
                    &mut state,
                    OfferTraceAction::RelayResult {
                        identity: id(identity),
                        relay: relay(target),
                        accepted: true,
                    },
                );
            }
            append(
                &mut trace,
                &mut state,
                OfferTraceAction::FinishAnnouncement {
                    identity: id(identity),
                },
            );
        }
        check_offer_trace(&trace, &OfferCheckerConfig::default()).unwrap();
    }

    #[test]
    fn duplicate_offer_for_another_identity_is_rejected() {
        let mut state = OfferAbstractState::default();
        let begin = |identity| OfferTraceAction::BeginAnnouncement {
            identity: id(identity),
            offer: offer("same"),
            target_relays: vec![relay("a")],
            event_kind: 10_058,
            author_matches: true,
            issuer_matches_wallet_owner: true,
        };
        state = apply_offer_action(&state, &begin("owner")).unwrap();
        state = apply_offer_action(
            &state,
            &OfferTraceAction::RelayResult {
                identity: id("owner"),
                relay: relay("a"),
                accepted: true,
            },
        )
        .unwrap();
        state = apply_offer_action(
            &state,
            &OfferTraceAction::FinishAnnouncement {
                identity: id("owner"),
            },
        )
        .unwrap();
        assert!(apply_offer_action(&state, &begin("agent")).is_err());
    }

    #[test]
    fn wrong_author_and_early_finish_are_rejected() {
        let state = OfferAbstractState::default();
        let wrong_author = OfferTraceAction::BeginWithdrawal {
            identity: id("agent"),
            target_relays: vec![relay("a")],
            event_kind: 10_058,
            author_matches: false,
        };
        assert!(apply_offer_action(&state, &wrong_author).is_err());

        let begun = apply_offer_action(
            &state,
            &OfferTraceAction::BeginWithdrawal {
                identity: id("agent"),
                target_relays: vec![relay("a")],
                event_kind: 10_058,
                author_matches: true,
            },
        )
        .unwrap();
        assert!(apply_offer_action(
            &begun,
            &OfferTraceAction::FinishWithdrawal {
                identity: id("agent")
            }
        )
        .is_err());
    }

    #[test]
    fn offer_from_non_owner_wallet_is_rejected() {
        let action = OfferTraceAction::BeginAnnouncement {
            identity: id("agent"),
            offer: offer("agent-wallet-offer"),
            target_relays: vec![relay("a")],
            event_kind: 10_058,
            author_matches: true,
            issuer_matches_wallet_owner: false,
        };
        assert!(apply_offer_action(&OfferAbstractState::default(), &action).is_err());
    }
}

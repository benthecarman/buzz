//! Independent runtime checker for `docs/spec/WalletPaymentAttempts.tla`.
//!
//! The desktop wallet emits this schema at its durable payment-attempt seam.
//! This module does not depend on desktop or provider code and does not call
//! the production transition helpers.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

/// Current wallet trace schema version.
pub const WALLET_TRACE_SCHEMA_VERSION: u32 = 1;

/// Stable, opaque identifier for one persisted payment attempt.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct WalletAttemptId(pub String);

/// The spec's `status` variable.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WalletAttemptStatus {
    /// No checkpoint exists for this attempt.
    Absent,
    /// A generic send is durable but has not reached the provider.
    GenericPrepared,
    /// A generic send may have reached the provider and may only reconcile.
    GenericPaying,
    /// A generic send completed.
    GenericCompleted,
    /// A generic send failed terminally.
    GenericFailed,
    /// A profile zap is durable but has not reached the provider.
    ProfilePrepared,
    /// A profile zap may have reached the provider and may only reconcile.
    ProfilePaying,
    /// A profile zap settled, but the provider supplied no publishable proof.
    ProfilePaidWithoutProof,
    /// A profile zap failed terminally.
    ProfileFailed,
}

/// Secret-free projection of one persisted payment attempt.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct WalletAbstractState {
    /// Durable attempt status.
    pub status: WalletAttemptStatus,
    /// Whether the checkpoint contains a provider payment result.
    pub payment_recorded: bool,
}

impl WalletAbstractState {
    /// Initial state for an attempt identifier not yet seen by the checker.
    pub const fn absent() -> Self {
        Self {
            status: WalletAttemptStatus::Absent,
            payment_recorded: false,
        }
    }
}

/// Critical decisions from `WalletPaymentAttempts.tla`'s `Next` relation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum WalletTraceAction {
    /// Persist a new generic send checkpoint.
    PrepareGeneric,
    /// Persist a new profile-zap checkpoint.
    PrepareProfile,
    /// Persist `Paying` before the first provider send call.
    BeginDispatch,
    /// Query the provider instead of issuing another send.
    Reconcile,
    /// A provider result is non-terminal; retain `Paying`.
    RecordPending,
    /// A generic provider payment completed.
    RecordCompleted,
    /// A profile payment settled without a publishable payer proof.
    RecordPaidWithoutProof,
    /// Record a terminal provider failure or an expired reconciliation.
    RecordFailed {
        /// True when the checkpoint includes a provider failure result; false
        /// when reconciliation expired without finding a payment.
        payment_recorded: bool,
    },
    /// Return an already-terminal attempt without contacting the provider.
    ReuseTerminal,
    /// Reject reuse of an attempt identifier with different request details.
    RejectConflict,
    /// Runtime witness that a critical seam exited without a known action.
    ImplBug,
}

impl WalletTraceAction {
    /// Stable action name used by coverage expectations and diagnostics.
    pub const fn kind(self) -> &'static str {
        match self {
            Self::PrepareGeneric => "prepare_generic",
            Self::PrepareProfile => "prepare_profile",
            Self::BeginDispatch => "begin_dispatch",
            Self::Reconcile => "reconcile",
            Self::RecordPending => "record_pending",
            Self::RecordCompleted => "record_completed",
            Self::RecordPaidWithoutProof => "record_paid_without_proof",
            Self::RecordFailed { .. } => "record_failed",
            Self::ReuseTerminal => "reuse_terminal",
            Self::RejectConflict => "reject_conflict",
            Self::ImplBug => "impl_bug",
        }
    }

    /// Every action at the payment-attempt seam is safety-critical.
    pub const fn is_critical(self) -> bool {
        true
    }
}

/// One JSONL trace step.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WalletTraceStep {
    /// Schema version.
    pub schema_version: u32,
    /// Opaque payment-attempt identifier.
    pub attempt_id: WalletAttemptId,
    /// Critical decision taken by the implementation.
    pub action: WalletTraceAction,
    /// Projection immediately before the decision.
    pub state_before: WalletAbstractState,
    /// Projection after durable persistence, and before any following side
    /// effect such as the provider send.
    pub state_after: WalletAbstractState,
}

impl WalletTraceStep {
    /// Construct a step at the current schema version.
    pub fn new(
        attempt_id: WalletAttemptId,
        action: WalletTraceAction,
        state_before: WalletAbstractState,
        state_after: WalletAbstractState,
    ) -> Self {
        Self {
            schema_version: WALLET_TRACE_SCHEMA_VERSION,
            attempt_id,
            action,
            state_before,
            state_after,
        }
    }
}

/// Scenario-specific coverage requirements.
#[derive(Debug, Clone, Default)]
pub struct WalletCheckerConfig {
    /// Critical action names the scenario must exercise.
    pub required_critical_actions: BTreeSet<String>,
}

impl WalletCheckerConfig {
    /// Add one required action kind.
    pub fn require(mut self, kind: &str) -> Self {
        self.required_critical_actions.insert(kind.to_string());
        self
    }
}

/// Replay failure with a human-readable counterexample.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum WalletCheckError {
    /// The action is forbidden from the checker-computed prior state.
    #[error("illegal wallet transition at step {step}: {reason}")]
    IllegalTransition {
        /// Zero-based trace index.
        step: usize,
        /// Rule that rejected the transition.
        reason: String,
    },
    /// The implementation projection differs from checker-computed state.
    #[error("wallet state mismatch at step {step}: expected {expected:?}, observed {observed:?}")]
    StateMismatch {
        /// Zero-based trace index.
        step: usize,
        /// Independently computed state.
        expected: WalletAbstractState,
        /// Implementation-emitted state.
        observed: WalletAbstractState,
    },
    /// A critical action was missing, unknown, malformed, or explicitly
    /// reported by the implementation as uncovered.
    #[error("wallet trace coverage breach: {reason}")]
    CoverageBreach {
        /// Coverage failure detail.
        reason: String,
    },
}

/// Replay trace steps against the independent translation of the spec.
pub fn check_wallet_trace(
    trace: &[WalletTraceStep],
    config: &WalletCheckerConfig,
) -> Result<(), WalletCheckError> {
    if trace.is_empty() {
        return Err(WalletCheckError::CoverageBreach {
            reason: "trace is empty".to_string(),
        });
    }

    let mut model = BTreeMap::<WalletAttemptId, WalletAbstractState>::new();
    let mut seen = BTreeSet::new();

    for (step_index, step) in trace.iter().enumerate() {
        if step.schema_version != WALLET_TRACE_SCHEMA_VERSION {
            return Err(WalletCheckError::CoverageBreach {
                reason: format!(
                    "step {step_index} uses schema {}, checker expects {}",
                    step.schema_version, WALLET_TRACE_SCHEMA_VERSION
                ),
            });
        }
        if step.action == WalletTraceAction::ImplBug {
            return Err(WalletCheckError::CoverageBreach {
                reason: format!("implementation reported an uncovered path at step {step_index}"),
            });
        }

        // Tracing is opt-in and can begin after a checkpoint was persisted.
        // Seed a newly observed attempt from the implementation's durable
        // projection; subsequent steps must chain from checker-computed state.
        let prior = if let Some(prior) = model.get(&step.attempt_id).copied() {
            prior
        } else {
            validate_persisted_state(step.state_before).map_err(|reason| {
                WalletCheckError::IllegalTransition {
                    step: step_index,
                    reason,
                }
            })?;
            step.state_before
        };
        if prior != step.state_before {
            return Err(WalletCheckError::StateMismatch {
                step: step_index,
                expected: prior,
                observed: step.state_before,
            });
        }

        let expected_after = apply_spec_action(prior, step.action).map_err(|reason| {
            WalletCheckError::IllegalTransition {
                step: step_index,
                reason,
            }
        })?;
        if expected_after != step.state_after {
            return Err(WalletCheckError::StateMismatch {
                step: step_index,
                expected: expected_after,
                observed: step.state_after,
            });
        }

        seen.insert(step.action.kind().to_string());
        model.insert(step.attempt_id.clone(), expected_after);
    }

    let missing: Vec<&str> = config
        .required_critical_actions
        .iter()
        .filter(|kind| !seen.contains(*kind))
        .map(String::as_str)
        .collect();
    if !missing.is_empty() {
        return Err(WalletCheckError::CoverageBreach {
            reason: format!("missing required critical actions: {missing:?}"),
        });
    }
    Ok(())
}

/// Parse JSONL and check it. Unknown or malformed actions fail as coverage
/// breaches instead of being silently skipped.
pub fn check_wallet_jsonl(
    jsonl: &str,
    config: &WalletCheckerConfig,
) -> Result<(), WalletCheckError> {
    let mut trace = Vec::new();
    for (line_index, line) in jsonl.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let step = serde_json::from_str::<WalletTraceStep>(line).map_err(|error| {
            WalletCheckError::CoverageBreach {
                reason: format!(
                    "unknown or malformed critical action on line {}: {error}",
                    line_index + 1
                ),
            }
        })?;
        trace.push(step);
    }
    check_wallet_trace(&trace, config)
}

fn apply_spec_action(
    before: WalletAbstractState,
    action: WalletTraceAction,
) -> Result<WalletAbstractState, String> {
    use WalletAttemptStatus as Status;
    use WalletTraceAction as Action;

    let unchanged = || Ok(before);
    match action {
        Action::PrepareGeneric if before == WalletAbstractState::absent() => {
            Ok(state(Status::GenericPrepared, false))
        }
        Action::PrepareProfile if before == WalletAbstractState::absent() => {
            Ok(state(Status::ProfilePrepared, false))
        }
        Action::BeginDispatch => match before.status {
            Status::GenericPrepared => Ok(state(Status::GenericPaying, false)),
            Status::ProfilePrepared => Ok(state(Status::ProfilePaying, false)),
            _ => illegal(before, action),
        },
        Action::Reconcile => match before.status {
            Status::GenericPaying | Status::ProfilePaying => unchanged(),
            _ => illegal(before, action),
        },
        Action::RecordPending => match before.status {
            Status::GenericPaying | Status::ProfilePaying => Ok(state(before.status, true)),
            _ => illegal(before, action),
        },
        Action::RecordCompleted if before.status == Status::GenericPaying => {
            Ok(state(Status::GenericCompleted, true))
        }
        Action::RecordPaidWithoutProof if before.status == Status::ProfilePaying => {
            Ok(state(Status::ProfilePaidWithoutProof, true))
        }
        Action::RecordFailed { payment_recorded } => match before.status {
            Status::GenericPaying if !before.payment_recorded || payment_recorded => {
                Ok(state(Status::GenericFailed, payment_recorded))
            }
            Status::ProfilePaying if !before.payment_recorded || payment_recorded => {
                Ok(state(Status::ProfileFailed, payment_recorded))
            }
            _ => illegal(before, action),
        },
        Action::ReuseTerminal => match before.status {
            Status::GenericCompleted
            | Status::GenericFailed
            | Status::ProfilePaidWithoutProof
            | Status::ProfileFailed => unchanged(),
            _ => illegal(before, action),
        },
        Action::RejectConflict if before.status != Status::Absent => unchanged(),
        Action::ImplBug => Err("impl_bug is a coverage breach".to_string()),
        _ => illegal(before, action),
    }
}

fn validate_persisted_state(state: WalletAbstractState) -> Result<(), String> {
    use WalletAttemptStatus as Status;

    let valid = match state.status {
        Status::Absent | Status::GenericPrepared | Status::ProfilePrepared => {
            !state.payment_recorded
        }
        Status::GenericCompleted | Status::ProfilePaidWithoutProof => state.payment_recorded,
        Status::GenericPaying
        | Status::GenericFailed
        | Status::ProfilePaying
        | Status::ProfileFailed => true,
    };

    if valid {
        Ok(())
    } else {
        Err(format!(
            "invalid persisted wallet state projection {state:?}"
        ))
    }
}

const fn state(status: WalletAttemptStatus, payment_recorded: bool) -> WalletAbstractState {
    WalletAbstractState {
        status,
        payment_recorded,
    }
}

fn illegal(
    before: WalletAbstractState,
    action: WalletTraceAction,
) -> Result<WalletAbstractState, String> {
    Err(format!(
        "action {} is forbidden from {before:?}",
        action.kind()
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id() -> WalletAttemptId {
        WalletAttemptId("attempt-1".to_string())
    }

    fn step(
        action: WalletTraceAction,
        before: WalletAbstractState,
        after: WalletAbstractState,
    ) -> WalletTraceStep {
        WalletTraceStep::new(id(), action, before, after)
    }

    #[test]
    fn valid_generic_execution_is_accepted() {
        let absent = WalletAbstractState::absent();
        let prepared = state(WalletAttemptStatus::GenericPrepared, false);
        let paying = state(WalletAttemptStatus::GenericPaying, false);
        let completed = state(WalletAttemptStatus::GenericCompleted, true);
        let trace = vec![
            step(WalletTraceAction::PrepareGeneric, absent, prepared),
            step(WalletTraceAction::BeginDispatch, prepared, paying),
            step(WalletTraceAction::RecordCompleted, paying, completed),
            step(WalletTraceAction::ReuseTerminal, completed, completed),
        ];
        check_wallet_trace(
            &trace,
            &WalletCheckerConfig::default()
                .require("begin_dispatch")
                .require("record_completed"),
        )
        .expect("valid execution");
    }

    #[test]
    fn completion_without_dispatch_is_illegal() {
        let prepared = state(WalletAttemptStatus::GenericPrepared, false);
        let completed = state(WalletAttemptStatus::GenericCompleted, true);
        let trace = vec![
            step(
                WalletTraceAction::PrepareGeneric,
                WalletAbstractState::absent(),
                prepared,
            ),
            step(WalletTraceAction::RecordCompleted, prepared, completed),
        ];
        assert!(matches!(
            check_wallet_trace(&trace, &WalletCheckerConfig::default()),
            Err(WalletCheckError::IllegalTransition { step: 1, .. })
        ));
    }

    #[test]
    fn projected_after_state_mismatch_is_rejected() {
        let trace = vec![step(
            WalletTraceAction::PrepareGeneric,
            WalletAbstractState::absent(),
            state(WalletAttemptStatus::GenericPaying, false),
        )];
        assert!(matches!(
            check_wallet_trace(&trace, &WalletCheckerConfig::default()),
            Err(WalletCheckError::StateMismatch { step: 0, .. })
        ));
    }

    #[test]
    fn first_observations_bootstrap_persisted_attempts() {
        let paying = state(WalletAttemptStatus::GenericPaying, false);
        let completed = state(WalletAttemptStatus::GenericCompleted, true);
        let prepared = state(WalletAttemptStatus::ProfilePrepared, false);
        let trace = vec![
            step(WalletTraceAction::Reconcile, paying, paying),
            WalletTraceStep::new(
                WalletAttemptId("attempt-2".to_string()),
                WalletTraceAction::ReuseTerminal,
                completed,
                completed,
            ),
            WalletTraceStep::new(
                WalletAttemptId("attempt-3".to_string()),
                WalletTraceAction::RejectConflict,
                prepared,
                prepared,
            ),
        ];

        check_wallet_trace(
            &trace,
            &WalletCheckerConfig::default()
                .require("reconcile")
                .require("reuse_terminal")
                .require("reject_conflict"),
        )
        .expect("persisted checkpoints should seed replay");
    }

    #[test]
    fn invalid_bootstrap_state_is_rejected() {
        let invalid = state(WalletAttemptStatus::GenericCompleted, false);
        let trace = vec![step(WalletTraceAction::ReuseTerminal, invalid, invalid)];

        assert!(matches!(
            check_wallet_trace(&trace, &WalletCheckerConfig::default()),
            Err(WalletCheckError::IllegalTransition { step: 0, .. })
        ));
    }

    #[test]
    fn missing_required_action_is_a_coverage_breach() {
        let trace = vec![step(
            WalletTraceAction::PrepareGeneric,
            WalletAbstractState::absent(),
            state(WalletAttemptStatus::GenericPrepared, false),
        )];
        assert!(matches!(
            check_wallet_trace(
                &trace,
                &WalletCheckerConfig::default().require("begin_dispatch")
            ),
            Err(WalletCheckError::CoverageBreach { .. })
        ));
    }

    #[test]
    fn unknown_json_action_is_a_coverage_breach() {
        let json = r#"{"schema_version":1,"attempt_id":"a","action":{"type":"send_again"},"state_before":{"status":"generic_paying","payment_recorded":false},"state_after":{"status":"generic_paying","payment_recorded":false}}"#;
        assert!(matches!(
            check_wallet_jsonl(json, &WalletCheckerConfig::default()),
            Err(WalletCheckError::CoverageBreach { .. })
        ));
    }
}

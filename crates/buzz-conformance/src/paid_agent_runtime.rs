//! Independent runtime checker for `docs/spec/PaidAgentRuntime.tla`.
//!
//! Identifiers are opaque and durations are integer milliseconds. This module
//! deliberately shares no reducer or authorization helper with `buzz-core` or
//! `buzz-acp`.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

/// Current paid-runtime trace schema version.
pub const PAID_RUNTIME_TRACE_SCHEMA_VERSION: u32 = 1;

/// Opaque identifier that contains no keys, event bodies, offers, or wallet data.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct RuntimeOpaqueId(pub String);

/// Terminal outcome projected by the runtime emitter.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeTraceOutcome {
    /// Successful completion.
    Completed,
    /// Provider or agent error.
    Error,
    /// Payer cancellation.
    Cancelled,
    /// Idle or hard timeout.
    Timeout,
    /// Full cap consumption.
    BudgetExhausted,
    /// Unexpected process loss.
    Interrupted,
    /// Admission expiry before execution.
    UnusedExpired,
}

/// Aggregate projection emitted before and after every critical action.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuntimeAbstractState {
    /// Verified deposited credit.
    pub credited_ms: u64,
    /// Settled billed usage.
    pub used_ms: u64,
    /// Caps locked by open reservations.
    pub locked_ms: u64,
    /// Number of open reservations.
    pub open_reservations: u64,
    /// Number of meters currently inside `session/prompt`.
    pub active_meters: u64,
}

/// Critical decisions from the paid-runtime lifecycle.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum RuntimeTraceAction {
    /// The Agent verified the scope's access mode, non-DM channel, and
    /// community before locking any credit for it. Under the prepaid
    /// protocol this happens at mint time — nothing precedes the payment.
    ScopeAuthorized {
        /// External access was authorized by the active access mode.
        ///
        /// The serialized field retains its version-1 `allowlisted` name for
        /// trace compatibility; `anyone` access also records this as true.
        allowlisted: bool,
        /// Channel is not a DM.
        non_dm: bool,
        /// Request and balance belong to this community.
        same_community: bool,
    },
    /// Wallet host independently verified an inbound settlement.
    PaymentSettled {
        /// Opaque zap-intent identifier.
        payment_id: RuntimeOpaqueId,
        /// Exact amount, offer, recipient, and payer note were verified.
        verified: bool,
    },
    /// One verified payment created runtime credit.
    CreditDeposited {
        /// Opaque zap-intent identifier.
        payment_id: RuntimeOpaqueId,
        /// Exact pack duration in milliseconds.
        credit_ms: u64,
    },
    /// A cap was atomically locked.
    RuntimeReserved {
        /// Opaque reservation event identifier.
        reservation_id: RuntimeOpaqueId,
        /// Locked cap in milliseconds.
        cap_ms: u64,
    },
    /// One instruction consumed one reservation admission.
    InstructionBound {
        /// Opaque reservation event identifier.
        reservation_id: RuntimeOpaqueId,
        /// Opaque instruction event identifier.
        instruction_id: RuntimeOpaqueId,
        /// External access was authorized by the active access mode.
        ///
        /// The serialized field retains its version-1 `allowlisted` name for
        /// trace compatibility; `anyone` access also records this as true.
        allowlisted: bool,
        /// Channel is not a DM.
        non_dm: bool,
        /// Instruction and reservation share a community.
        same_community: bool,
    },
    /// Dispatch revalidated the signed paid marker and its open reservation.
    InvocationDispatched {
        /// Opaque reservation event identifier.
        reservation_id: RuntimeOpaqueId,
        /// Opaque instruction event identifier.
        instruction_id: RuntimeOpaqueId,
    },
    /// Monotonic metering began immediately before `session/prompt`.
    MeterStarted {
        /// Opaque reservation event identifier.
        reservation_id: RuntimeOpaqueId,
    },
    /// Durable monotonic elapsed time was advanced.
    MeterCheckpointed {
        /// Opaque reservation event identifier.
        reservation_id: RuntimeOpaqueId,
        /// Accumulated billable milliseconds.
        elapsed_ms: u64,
    },
    /// Metering paused outside `session/prompt`, such as retry backoff.
    MeterPaused {
        /// Opaque reservation event identifier.
        reservation_id: RuntimeOpaqueId,
    },
    /// Metering resumed for another prompt segment.
    MeterResumed {
        /// Opaque reservation event identifier.
        reservation_id: RuntimeOpaqueId,
    },
    /// Reservation lock was replaced by final measured usage.
    ReservationSettled {
        /// Opaque reservation event identifier.
        reservation_id: RuntimeOpaqueId,
        /// Final billed milliseconds.
        used_ms: u64,
        /// Terminal outcome.
        outcome: RuntimeTraceOutcome,
    },
    /// An exact duplicate reused its existing durable effect.
    DuplicateReused {
        /// Opaque payment or reservation identifier.
        entity_id: RuntimeOpaqueId,
    },
    /// A request was generically rejected before invocation.
    InvocationRejected,
    /// Runtime witness that a critical seam exited without a modeled action.
    ImplBug,
}

impl RuntimeTraceAction {
    /// Stable action name for coverage requirements.
    pub const fn kind(&self) -> &'static str {
        match self {
            Self::ScopeAuthorized { .. } => "scope_authorized",
            Self::PaymentSettled { .. } => "payment_settled",
            Self::CreditDeposited { .. } => "credit_deposited",
            Self::RuntimeReserved { .. } => "runtime_reserved",
            Self::InstructionBound { .. } => "instruction_bound",
            Self::InvocationDispatched { .. } => "invocation_dispatched",
            Self::MeterStarted { .. } => "meter_started",
            Self::MeterCheckpointed { .. } => "meter_checkpointed",
            Self::MeterPaused { .. } => "meter_paused",
            Self::MeterResumed { .. } => "meter_resumed",
            Self::ReservationSettled { .. } => "reservation_settled",
            Self::DuplicateReused { .. } => "duplicate_reused",
            Self::InvocationRejected => "invocation_rejected",
            Self::ImplBug => "impl_bug",
        }
    }
}

/// One JSONL trace step for one payer-agent-community scope.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuntimeTraceStep {
    /// Schema version.
    pub schema_version: u32,
    /// Opaque identifier for the payer-agent-community tuple.
    pub scope_id: RuntimeOpaqueId,
    /// Critical implementation decision.
    pub action: RuntimeTraceAction,
    /// Projection immediately before the decision.
    pub state_before: RuntimeAbstractState,
    /// Projection after durable persistence.
    pub state_after: RuntimeAbstractState,
}

/// Scenario-specific critical-action coverage.
#[derive(Debug, Clone, Default)]
pub struct RuntimeCheckerConfig {
    /// Action names that must occur.
    pub required_critical_actions: BTreeSet<String>,
}

impl RuntimeCheckerConfig {
    /// Require an action kind in the checked scenario.
    pub fn require(mut self, kind: &str) -> Self {
        self.required_critical_actions.insert(kind.to_string());
        self
    }
}

/// Replay failure with a minimal counterexample.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RuntimeCheckError {
    /// Action is forbidden from the checker-computed state.
    #[error("illegal paid-runtime transition at step {step}: {reason}")]
    IllegalTransition {
        /// Zero-based trace index.
        step: usize,
        /// Rejected invariant or guard.
        reason: String,
    },
    /// Emitter projection differs from independently computed state.
    #[error(
        "paid-runtime state mismatch at step {step}: expected {expected:?}, observed {observed:?}"
    )]
    StateMismatch {
        /// Zero-based trace index.
        step: usize,
        /// Checker-computed state.
        expected: RuntimeAbstractState,
        /// Emitter-projected state.
        observed: RuntimeAbstractState,
    },
    /// Critical seam coverage is incomplete or malformed.
    #[error("paid-runtime trace coverage breach: {reason}")]
    CoverageBreach {
        /// Coverage failure detail.
        reason: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MeterState {
    Idle,
    Active,
    Paused,
}

#[derive(Debug, Clone)]
struct ReservationModel {
    cap_ms: u64,
    instruction_id: Option<RuntimeOpaqueId>,
    dispatched: bool,
    meter: MeterState,
    checkpoint_ms: u64,
    settlement: Option<(u64, RuntimeTraceOutcome)>,
}

#[derive(Debug, Default)]
struct ScopeModel {
    scope_authorized: bool,
    settled_payments: BTreeSet<RuntimeOpaqueId>,
    deposits: BTreeMap<RuntimeOpaqueId, u64>,
    reservations: BTreeMap<RuntimeOpaqueId, ReservationModel>,
}

impl ScopeModel {
    fn state(&self) -> Result<RuntimeAbstractState, &'static str> {
        let credited_ms = checked_sum(self.deposits.values().copied())?;
        let used_ms = checked_sum(
            self.reservations
                .values()
                .filter_map(|reservation| reservation.settlement.map(|value| value.0)),
        )?;
        let locked_ms = checked_sum(
            self.reservations
                .values()
                .filter(|reservation| reservation.settlement.is_none())
                .map(|reservation| reservation.cap_ms),
        )?;
        let open_reservations = self
            .reservations
            .values()
            .filter(|reservation| reservation.settlement.is_none())
            .count() as u64;
        let active_meters = self
            .reservations
            .values()
            .filter(|reservation| reservation.meter == MeterState::Active)
            .count() as u64;
        credited_ms
            .checked_sub(used_ms)
            .and_then(|value| value.checked_sub(locked_ms))
            .ok_or("ledger totals underflow")?;
        Ok(RuntimeAbstractState {
            credited_ms,
            used_ms,
            locked_ms,
            open_reservations,
            active_meters,
        })
    }

    fn apply(&mut self, action: &RuntimeTraceAction) -> Result<(), String> {
        match action {
            RuntimeTraceAction::ScopeAuthorized {
                allowlisted,
                non_dm,
                same_community,
            } => {
                if !(*allowlisted && *non_dm && *same_community) {
                    return Err(
                        "scope authorized without access, channel, and community checks".into(),
                    );
                }
                self.scope_authorized = true;
            }
            RuntimeTraceAction::PaymentSettled {
                payment_id,
                verified,
            } => {
                if !verified {
                    return Err("payment settlement was not independently verified".into());
                }
                self.settled_payments.insert(payment_id.clone());
            }
            RuntimeTraceAction::CreditDeposited {
                payment_id,
                credit_ms,
            } => {
                if *credit_ms == 0 || !self.settled_payments.contains(payment_id) {
                    return Err("credit requires a nonzero verified settlement".into());
                }
                match self.deposits.get(payment_id) {
                    Some(existing) if existing == credit_ms => {}
                    Some(_) => return Err("payment identifier changed its credit effect".into()),
                    None => {
                        self.deposits.insert(payment_id.clone(), *credit_ms);
                    }
                }
            }
            RuntimeTraceAction::RuntimeReserved {
                reservation_id,
                cap_ms,
            } => {
                if *cap_ms == 0 {
                    return Err("reservation cap is zero".into());
                }
                if !self.scope_authorized {
                    return Err("reservation minted before scope authorization checks".into());
                }
                if let Some(existing) = self.reservations.get(reservation_id) {
                    if existing.cap_ms != *cap_ms {
                        return Err("reservation identifier changed its cap".into());
                    }
                    return Ok(());
                }
                let state = self.state().map_err(str::to_string)?;
                let available = state
                    .credited_ms
                    .checked_sub(state.used_ms)
                    .and_then(|value| value.checked_sub(state.locked_ms))
                    .ok_or_else(|| "ledger totals underflow".to_string())?;
                if available < *cap_ms {
                    return Err("reservation overspends available runtime".into());
                }
                self.reservations.insert(
                    reservation_id.clone(),
                    ReservationModel {
                        cap_ms: *cap_ms,
                        instruction_id: None,
                        dispatched: false,
                        meter: MeterState::Idle,
                        checkpoint_ms: 0,
                        settlement: None,
                    },
                );
            }
            RuntimeTraceAction::InstructionBound {
                reservation_id,
                instruction_id,
                allowlisted,
                non_dm,
                same_community,
            } => {
                if !(*allowlisted && *non_dm && *same_community) {
                    return Err("external invocation failed authorization or scope checks".into());
                }
                if self
                    .reservations
                    .values()
                    .any(|reservation| reservation.instruction_id.as_ref() == Some(instruction_id))
                {
                    return Err("instruction is already bound to a reservation".into());
                }
                let reservation = open_reservation_mut(&mut self.reservations, reservation_id)?;
                if reservation.instruction_id.is_some() {
                    return Err("reservation is already consumed by an instruction".into());
                }
                reservation.instruction_id = Some(instruction_id.clone());
            }
            RuntimeTraceAction::InvocationDispatched {
                reservation_id,
                instruction_id,
            } => {
                let reservation = open_reservation_mut(&mut self.reservations, reservation_id)?;
                if reservation.instruction_id.as_ref() != Some(instruction_id) {
                    return Err("dispatch does not match the bound instruction".into());
                }
                if reservation.dispatched {
                    return Err("reservation was dispatched more than once".into());
                }
                reservation.dispatched = true;
            }
            RuntimeTraceAction::MeterStarted { reservation_id }
            | RuntimeTraceAction::MeterResumed { reservation_id } => {
                let reservation = open_reservation_mut(&mut self.reservations, reservation_id)?;
                if !reservation.dispatched
                    || reservation.meter == MeterState::Active
                    || reservation.checkpoint_ms >= reservation.cap_ms
                {
                    return Err("meter cannot start outside an admitted prompt segment".into());
                }
                reservation.meter = MeterState::Active;
            }
            RuntimeTraceAction::MeterCheckpointed {
                reservation_id,
                elapsed_ms,
            } => {
                let reservation = open_reservation_mut(&mut self.reservations, reservation_id)?;
                if reservation.meter != MeterState::Active
                    || *elapsed_ms < reservation.checkpoint_ms
                    || *elapsed_ms > reservation.cap_ms
                {
                    return Err("checkpoint is outside an active meter or cap".into());
                }
                reservation.checkpoint_ms = *elapsed_ms;
            }
            RuntimeTraceAction::MeterPaused { reservation_id } => {
                let reservation = open_reservation_mut(&mut self.reservations, reservation_id)?;
                if reservation.meter != MeterState::Active {
                    return Err("only an active meter can pause".into());
                }
                reservation.meter = MeterState::Paused;
            }
            RuntimeTraceAction::ReservationSettled {
                reservation_id,
                used_ms,
                outcome,
            } => {
                let reservation = open_reservation_mut(&mut self.reservations, reservation_id)?;
                if *used_ms > reservation.cap_ms {
                    return Err("settlement exceeds reservation cap".into());
                }
                if *outcome == RuntimeTraceOutcome::BudgetExhausted
                    && *used_ms != reservation.cap_ms
                {
                    return Err("budget exhaustion must bill exactly the cap".into());
                }
                if *outcome == RuntimeTraceOutcome::Interrupted
                    && *used_ms != reservation.checkpoint_ms
                {
                    return Err("crash recovery must use the last durable checkpoint".into());
                }
                if *outcome == RuntimeTraceOutcome::UnusedExpired && *used_ms != 0 {
                    return Err("unused expiry must return the full cap".into());
                }
                if *used_ms < reservation.checkpoint_ms {
                    return Err("settlement precedes the durable checkpoint".into());
                }
                reservation.meter = MeterState::Idle;
                reservation.settlement = Some((*used_ms, *outcome));
            }
            RuntimeTraceAction::DuplicateReused { entity_id } => {
                if !self.deposits.contains_key(entity_id)
                    && !self.reservations.contains_key(entity_id)
                    && !self.settled_payments.contains(entity_id)
                {
                    return Err("duplicate reuse references no durable effect".into());
                }
            }
            RuntimeTraceAction::InvocationRejected => {}
            RuntimeTraceAction::ImplBug => {
                return Err("implementation reported an uncovered critical exit".into())
            }
        }
        Ok(())
    }
}

fn open_reservation_mut<'a>(
    reservations: &'a mut BTreeMap<RuntimeOpaqueId, ReservationModel>,
    id: &RuntimeOpaqueId,
) -> Result<&'a mut ReservationModel, String> {
    let reservation = reservations
        .get_mut(id)
        .ok_or_else(|| "reservation does not exist".to_string())?;
    if reservation.settlement.is_some() {
        return Err("reservation is already settled".into());
    }
    Ok(reservation)
}

fn checked_sum(mut values: impl Iterator<Item = u64>) -> Result<u64, &'static str> {
    values
        .try_fold(0u64, |sum, value| sum.checked_add(value))
        .ok_or("duration arithmetic overflow")
}

/// Replay a paid-runtime trace against an independent transition system.
pub fn check_runtime_trace(
    trace: &[RuntimeTraceStep],
    config: &RuntimeCheckerConfig,
) -> Result<(), RuntimeCheckError> {
    if trace.is_empty() {
        return Err(RuntimeCheckError::CoverageBreach {
            reason: "trace is empty".into(),
        });
    }
    let mut scopes = BTreeMap::<RuntimeOpaqueId, ScopeModel>::new();
    let mut seen_actions = BTreeSet::new();
    for (index, step) in trace.iter().enumerate() {
        if step.schema_version != PAID_RUNTIME_TRACE_SCHEMA_VERSION {
            return Err(RuntimeCheckError::CoverageBreach {
                reason: format!("step {index} has an unsupported schema version"),
            });
        }
        if step.action == RuntimeTraceAction::ImplBug {
            return Err(RuntimeCheckError::CoverageBreach {
                reason: format!("step {index} reports impl_bug"),
            });
        }
        let model = scopes.entry(step.scope_id.clone()).or_default();
        let expected_before =
            model
                .state()
                .map_err(|reason| RuntimeCheckError::IllegalTransition {
                    step: index,
                    reason: reason.into(),
                })?;
        if step.state_before != expected_before {
            return Err(RuntimeCheckError::StateMismatch {
                step: index,
                expected: expected_before,
                observed: step.state_before,
            });
        }
        model
            .apply(&step.action)
            .map_err(|reason| RuntimeCheckError::IllegalTransition {
                step: index,
                reason,
            })?;
        let expected_after =
            model
                .state()
                .map_err(|reason| RuntimeCheckError::IllegalTransition {
                    step: index,
                    reason: reason.into(),
                })?;
        if step.state_after != expected_after {
            return Err(RuntimeCheckError::StateMismatch {
                step: index,
                expected: expected_after,
                observed: step.state_after,
            });
        }
        seen_actions.insert(step.action.kind().to_string());
    }
    let missing = config
        .required_critical_actions
        .difference(&seen_actions)
        .cloned()
        .collect::<Vec<_>>();
    if !missing.is_empty() {
        return Err(RuntimeCheckError::CoverageBreach {
            reason: format!("missing required actions: {}", missing.join(", ")),
        });
    }
    Ok(())
}

/// Decode and check newline-delimited paid-runtime trace steps.
pub fn check_runtime_jsonl(
    jsonl: &str,
    config: &RuntimeCheckerConfig,
) -> Result<(), RuntimeCheckError> {
    let mut trace = Vec::new();
    for (line_index, line) in jsonl.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let step =
            serde_json::from_str(line).map_err(|error| RuntimeCheckError::CoverageBreach {
                reason: format!("invalid JSONL at line {}: {error}", line_index + 1),
            })?;
        trace.push(step);
    }
    check_runtime_trace(&trace, config)
}

//! Secret-free implementation trace emitter for paid runtime conformance.

use std::{
    collections::{BTreeMap, BTreeSet},
    fs::OpenOptions,
    io::Write,
    sync::{Mutex, OnceLock},
};

use buzz_conformance::paid_agent_runtime::{
    RuntimeAbstractState, RuntimeOpaqueId, RuntimeTraceAction, RuntimeTraceOutcome,
    RuntimeTraceStep, PAID_RUNTIME_TRACE_SCHEMA_VERSION,
};
use sha2::{Digest, Sha256};

#[derive(Default)]
struct ReservationProjection {
    cap_ms: u64,
    used_ms: Option<u64>,
    active: bool,
    checkpoint_ms: u64,
    bound: bool,
    dispatched: bool,
}

#[derive(Default)]
struct ScopeProjection {
    deposits: BTreeMap<RuntimeOpaqueId, u64>,
    reservations: BTreeMap<RuntimeOpaqueId, ReservationProjection>,
}

impl ScopeProjection {
    fn state(&self) -> RuntimeAbstractState {
        RuntimeAbstractState {
            credited_ms: self
                .deposits
                .values()
                .copied()
                .fold(0u64, u64::saturating_add),
            used_ms: self
                .reservations
                .values()
                .filter_map(|reservation| reservation.used_ms)
                .fold(0u64, u64::saturating_add),
            locked_ms: self
                .reservations
                .values()
                .filter(|reservation| reservation.used_ms.is_none())
                .map(|reservation| reservation.cap_ms)
                .fold(0u64, u64::saturating_add),
            open_reservations: self
                .reservations
                .values()
                .filter(|reservation| reservation.used_ms.is_none())
                .count() as u64,
            active_meters: self
                .reservations
                .values()
                .filter(|reservation| reservation.active && reservation.used_ms.is_none())
                .count() as u64,
        }
    }

    fn apply(&mut self, action: &RuntimeTraceAction) {
        match action {
            RuntimeTraceAction::CreditDeposited {
                payment_id,
                credit_ms,
            } => {
                self.deposits.insert(payment_id.clone(), *credit_ms);
            }
            RuntimeTraceAction::RuntimeReserved {
                reservation_id,
                cap_ms,
            } => {
                self.reservations
                    .entry(reservation_id.clone())
                    .or_insert_with(|| ReservationProjection {
                        cap_ms: *cap_ms,
                        ..ReservationProjection::default()
                    });
            }
            RuntimeTraceAction::MeterStarted { reservation_id }
            | RuntimeTraceAction::MeterResumed { reservation_id } => {
                if let Some(reservation) = self.reservations.get_mut(reservation_id) {
                    reservation.active = true;
                }
            }
            RuntimeTraceAction::InstructionBound { reservation_id, .. } => {
                if let Some(reservation) = self.reservations.get_mut(reservation_id) {
                    reservation.bound = true;
                }
            }
            RuntimeTraceAction::InvocationDispatched { reservation_id, .. } => {
                if let Some(reservation) = self.reservations.get_mut(reservation_id) {
                    reservation.dispatched = true;
                }
            }
            RuntimeTraceAction::MeterCheckpointed {
                reservation_id,
                elapsed_ms,
            } => {
                if let Some(reservation) = self.reservations.get_mut(reservation_id) {
                    reservation.checkpoint_ms = *elapsed_ms;
                }
            }
            RuntimeTraceAction::MeterPaused { reservation_id } => {
                if let Some(reservation) = self.reservations.get_mut(reservation_id) {
                    reservation.active = false;
                }
            }
            RuntimeTraceAction::ReservationSettled {
                reservation_id,
                used_ms,
                ..
            } => {
                if let Some(reservation) = self.reservations.get_mut(reservation_id) {
                    reservation.active = false;
                    reservation.used_ms = Some(*used_ms);
                }
            }
            _ => {}
        }
    }
}

#[derive(Default)]
struct TraceState {
    initialized: BTreeSet<RuntimeOpaqueId>,
    scopes: BTreeMap<RuntimeOpaqueId, ScopeProjection>,
}

static TRACE_STATE: OnceLock<Mutex<TraceState>> = OnceLock::new();
static TRACE_SINK: OnceLock<std::io::Result<Mutex<std::fs::File>>> = OnceLock::new();

fn opaque(namespace: &[u8], values: &[&str]) -> RuntimeOpaqueId {
    let mut hasher = Sha256::new();
    hasher.update(b"buzz-paid-runtime-trace-v1\0");
    hasher.update(namespace);
    for value in values {
        hasher.update(b"\0");
        hasher.update(value.as_bytes());
    }
    RuntimeOpaqueId(hex::encode(&hasher.finalize()[..16]))
}

pub fn scope_id(agent: &str, payer: &str, channel: &str) -> RuntimeOpaqueId {
    opaque(b"scope", &[agent, payer, channel])
}

pub fn entity_id(kind: &str, raw: &str) -> RuntimeOpaqueId {
    opaque(kind.as_bytes(), &[raw])
}

fn enabled() -> bool {
    std::env::var_os("BUZZ_PAID_RUNTIME_TRACE_PATH").is_some()
}

pub fn record(scope_id: RuntimeOpaqueId, action: RuntimeTraceAction) {
    if !enabled() {
        return;
    }
    let state = TRACE_STATE.get_or_init(|| Mutex::new(TraceState::default()));
    let Ok(mut state) = state.lock() else {
        tracing::warn!("paid runtime trace state lock is poisoned");
        return;
    };
    let projection = state.scopes.entry(scope_id.clone()).or_default();
    let state_before = projection.state();
    projection.apply(&action);
    let state_after = projection.state();
    let step = RuntimeTraceStep {
        schema_version: PAID_RUNTIME_TRACE_SCHEMA_VERSION,
        scope_id,
        action,
        state_before,
        state_after,
    };
    drop(state);

    let Some(path) = std::env::var_os("BUZZ_PAID_RUNTIME_TRACE_PATH") else {
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
        tracing::warn!("open BUZZ_PAID_RUNTIME_TRACE_PATH failed");
        return;
    };
    let Ok(mut file) = sink.lock() else {
        tracing::warn!("paid runtime trace sink lock is poisoned");
        return;
    };
    let Ok(mut line) = serde_json::to_vec(&step) else {
        tracing::warn!("serialize paid runtime trace step failed");
        return;
    };
    line.push(b'\n');
    if let Err(error) = file.write_all(&line).and_then(|()| file.flush()) {
        tracing::warn!(%error, "write paid runtime trace step failed");
    }
}

pub fn record_binding(scope: RuntimeOpaqueId, reservation: &str, instruction: &str) {
    if !enabled() {
        return;
    }
    let reservation_id = entity_id("reservation", reservation);
    let already_bound = match TRACE_STATE
        .get_or_init(|| Mutex::new(TraceState::default()))
        .lock()
    {
        Ok(state) => state
            .scopes
            .get(&scope)
            .and_then(|projection| projection.reservations.get(&reservation_id))
            .is_some_and(|projection| projection.bound),
        Err(_) => false,
    };
    if already_bound {
        record(
            scope,
            RuntimeTraceAction::DuplicateReused {
                entity_id: reservation_id,
            },
        );
    } else {
        record(
            scope,
            RuntimeTraceAction::InstructionBound {
                reservation_id,
                instruction_id: entity_id("instruction", instruction),
                allowlisted: true,
                non_dm: true,
                same_community: true,
            },
        );
    }
}

pub fn record_dispatch(scope: RuntimeOpaqueId, reservation: &str, instruction: &str) {
    if !enabled() {
        return;
    }
    let reservation_id = entity_id("reservation", reservation);
    let already_dispatched = match TRACE_STATE
        .get_or_init(|| Mutex::new(TraceState::default()))
        .lock()
    {
        Ok(state) => state
            .scopes
            .get(&scope)
            .and_then(|projection| projection.reservations.get(&reservation_id))
            .is_some_and(|projection| projection.dispatched),
        Err(_) => false,
    };
    if already_dispatched {
        record(
            scope,
            RuntimeTraceAction::DuplicateReused {
                entity_id: reservation_id,
            },
        );
    } else {
        record(
            scope,
            RuntimeTraceAction::InvocationDispatched {
                reservation_id,
                instruction_id: entity_id("instruction", instruction),
            },
        );
    }
}

/// Initialize one scope exactly once in this process. The caller must replay
/// its durable ledger in `(created_at, event_id)` order when this returns true.
pub fn begin_scope(scope: RuntimeOpaqueId) -> bool {
    if !enabled() {
        return false;
    }
    let state = TRACE_STATE.get_or_init(|| Mutex::new(TraceState::default()));
    let Ok(mut state) = state.lock() else {
        return false;
    };
    let inserted = state.initialized.insert(scope.clone());
    drop(state);
    if inserted {
        record(
            scope,
            RuntimeTraceAction::QuoteRequested {
                allowlisted: true,
                non_dm: true,
                same_community: true,
            },
        );
    }
    inserted
}

#[allow(dead_code)]
pub fn initialize_scope(
    scope: RuntimeOpaqueId,
    deposits: &[(String, u64)],
    reservations: &[(String, u64)],
    settlements: &[(String, u64, RuntimeTraceOutcome)],
) {
    if !enabled() {
        return;
    }
    let state = TRACE_STATE.get_or_init(|| Mutex::new(TraceState::default()));
    let Ok(mut guard) = state.lock() else {
        return;
    };
    if !guard.initialized.insert(scope.clone()) {
        return;
    }
    drop(guard);
    record(
        scope.clone(),
        RuntimeTraceAction::QuoteRequested {
            allowlisted: true,
            non_dm: true,
            same_community: true,
        },
    );
    for (payment, credit_ms) in deposits {
        let payment_id = entity_id("payment", payment);
        record(
            scope.clone(),
            RuntimeTraceAction::PaymentSettled {
                payment_id: payment_id.clone(),
                verified: true,
            },
        );
        record(
            scope.clone(),
            RuntimeTraceAction::CreditDeposited {
                payment_id,
                credit_ms: *credit_ms,
            },
        );
    }
    for (reservation, cap_ms) in reservations {
        record(
            scope.clone(),
            RuntimeTraceAction::RuntimeReserved {
                reservation_id: entity_id("reservation", reservation),
                cap_ms: *cap_ms,
            },
        );
    }
    for (reservation, used_ms, outcome) in settlements {
        let reservation_id = entity_id("reservation", reservation);
        if *used_ms > 0 {
            record(
                scope.clone(),
                RuntimeTraceAction::InstructionBound {
                    reservation_id: reservation_id.clone(),
                    instruction_id: entity_id("bootstrap-instruction", reservation),
                    allowlisted: true,
                    non_dm: true,
                    same_community: true,
                },
            );
            record(
                scope.clone(),
                RuntimeTraceAction::InvocationDispatched {
                    reservation_id: reservation_id.clone(),
                    instruction_id: entity_id("bootstrap-instruction", reservation),
                },
            );
            record(
                scope.clone(),
                RuntimeTraceAction::MeterStarted {
                    reservation_id: reservation_id.clone(),
                },
            );
            record(
                scope.clone(),
                RuntimeTraceAction::MeterCheckpointed {
                    reservation_id: reservation_id.clone(),
                    elapsed_ms: *used_ms,
                },
            );
        }
        record(
            scope.clone(),
            RuntimeTraceAction::ReservationSettled {
                reservation_id,
                used_ms: *used_ms,
                outcome: *outcome,
            },
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use buzz_conformance::paid_agent_runtime::{check_runtime_jsonl, RuntimeCheckerConfig};

    #[test]
    fn production_emitter_trace_is_accepted_by_independent_checker() {
        let path = std::env::temp_dir().join(format!(
            "buzz-paid-runtime-production-trace-{}.jsonl",
            uuid::Uuid::new_v4()
        ));
        std::env::set_var("BUZZ_PAID_RUNTIME_TRACE_PATH", &path);
        let scope = scope_id("agent", "payer", "community");
        initialize_scope(
            scope.clone(),
            &[("payment".into(), 120_000)],
            &[("reservation".into(), 60_000)],
            &[],
        );
        record_binding(scope.clone(), "reservation", "instruction");
        record_dispatch(scope.clone(), "reservation", "instruction");
        let reservation_id = entity_id("reservation", "reservation");
        record(
            scope.clone(),
            RuntimeTraceAction::MeterStarted {
                reservation_id: reservation_id.clone(),
            },
        );
        record(
            scope.clone(),
            RuntimeTraceAction::MeterCheckpointed {
                reservation_id: reservation_id.clone(),
                elapsed_ms: 41_237,
            },
        );
        record(
            scope,
            RuntimeTraceAction::ReservationSettled {
                reservation_id,
                used_ms: 41_237,
                outcome: RuntimeTraceOutcome::Completed,
            },
        );

        let jsonl = std::fs::read_to_string(&path).expect("read emitted runtime trace");
        let config = RuntimeCheckerConfig::default()
            .require("payment_settled")
            .require("credit_deposited")
            .require("runtime_reserved")
            .require("instruction_bound")
            .require("invocation_dispatched")
            .require("meter_started")
            .require("meter_checkpointed")
            .require("reservation_settled");
        check_runtime_jsonl(&jsonl, &config).expect("production trace must conform");
        let _ = std::fs::remove_file(path);
    }
}

use buzz_conformance::paid_agent_runtime::{
    check_runtime_trace, RuntimeAbstractState, RuntimeCheckerConfig, RuntimeOpaqueId,
    RuntimeTraceAction, RuntimeTraceOutcome, RuntimeTraceStep, PAID_RUNTIME_TRACE_SCHEMA_VERSION,
};
use proptest::prelude::*;

fn step(
    action: RuntimeTraceAction,
    state_before: RuntimeAbstractState,
    state_after: RuntimeAbstractState,
) -> RuntimeTraceStep {
    RuntimeTraceStep {
        schema_version: PAID_RUNTIME_TRACE_SCHEMA_VERSION,
        scope_id: RuntimeOpaqueId("scope".into()),
        action,
        state_before,
        state_after,
    }
}

proptest! {
    #[test]
    fn valid_credit_reserve_and_settle_never_underflows(
        cap_ms in 1u64..1_000_000,
        extra_ms in 0u64..1_000_000,
        used_seed in any::<u64>(),
    ) {
        let credit_ms = cap_ms + extra_ms;
        let used_ms = used_seed % (cap_ms + 1);
        let zero = RuntimeAbstractState::default();
        let credited = RuntimeAbstractState { credited_ms: credit_ms, ..zero };
        let locked = RuntimeAbstractState {
            credited_ms: credit_ms,
            locked_ms: cap_ms,
            open_reservations: 1,
            ..zero
        };
        let active = RuntimeAbstractState { active_meters: 1, ..locked };
        let settled = RuntimeAbstractState { credited_ms: credit_ms, used_ms, ..zero };
        let trace = vec![
            step(RuntimeTraceAction::QuoteRequested { allowlisted: true, non_dm: true, same_community: true }, zero, zero),
            step(RuntimeTraceAction::PaymentSettled { payment_id: RuntimeOpaqueId("payment".into()), verified: true }, zero, zero),
            step(RuntimeTraceAction::CreditDeposited { payment_id: RuntimeOpaqueId("payment".into()), credit_ms }, zero, credited),
            step(RuntimeTraceAction::RuntimeReserved { reservation_id: RuntimeOpaqueId("reservation".into()), cap_ms }, credited, locked),
            step(RuntimeTraceAction::InstructionBound {
                reservation_id: RuntimeOpaqueId("reservation".into()),
                instruction_id: RuntimeOpaqueId("instruction".into()),
                allowlisted: true,
                non_dm: true,
                same_community: true,
            }, locked, locked),
            step(RuntimeTraceAction::InvocationDispatched {
                reservation_id: RuntimeOpaqueId("reservation".into()),
                instruction_id: RuntimeOpaqueId("instruction".into()),
            }, locked, locked),
            step(RuntimeTraceAction::MeterStarted { reservation_id: RuntimeOpaqueId("reservation".into()) }, locked, active),
            step(RuntimeTraceAction::ReservationSettled {
                reservation_id: RuntimeOpaqueId("reservation".into()),
                used_ms,
                outcome: RuntimeTraceOutcome::Completed,
            }, active, settled),
        ];
        prop_assert!(check_runtime_trace(&trace, &RuntimeCheckerConfig::default()).is_ok());
    }
}

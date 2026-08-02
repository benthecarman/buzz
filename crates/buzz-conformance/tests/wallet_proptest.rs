use buzz_conformance::wallet::{
    check_wallet_trace, WalletAbstractState, WalletAttemptId, WalletAttemptStatus,
    WalletCheckError, WalletCheckerConfig, WalletTraceAction, WalletTraceStep,
};
use proptest::prelude::*;

fn state(status: WalletAttemptStatus, payment_recorded: bool) -> WalletAbstractState {
    WalletAbstractState {
        status,
        payment_recorded,
    }
}

fn generic_trace(pending_observations: usize, failed: bool) -> Vec<WalletTraceStep> {
    let id = WalletAttemptId("generated-generic".to_string());
    let absent = WalletAbstractState::absent();
    let prepared = state(WalletAttemptStatus::GenericPrepared, false);
    let paying_empty = state(WalletAttemptStatus::GenericPaying, false);
    let paying_observed = state(WalletAttemptStatus::GenericPaying, true);
    let mut trace = vec![
        WalletTraceStep::new(
            id.clone(),
            WalletTraceAction::PrepareGeneric,
            absent,
            prepared,
        ),
        WalletTraceStep::new(
            id.clone(),
            WalletTraceAction::BeginDispatch,
            prepared,
            paying_empty,
        ),
    ];
    let mut before = paying_empty;
    for _ in 0..pending_observations {
        trace.push(WalletTraceStep::new(
            id.clone(),
            WalletTraceAction::Reconcile,
            before,
            before,
        ));
        trace.push(WalletTraceStep::new(
            id.clone(),
            WalletTraceAction::RecordPending,
            before,
            paying_observed,
        ));
        before = paying_observed;
    }
    let (action, after) = if failed {
        (
            WalletTraceAction::RecordFailed {
                payment_recorded: true,
            },
            state(WalletAttemptStatus::GenericFailed, true),
        )
    } else {
        (
            WalletTraceAction::RecordCompleted,
            state(WalletAttemptStatus::GenericCompleted, true),
        )
    };
    trace.push(WalletTraceStep::new(id.clone(), action, before, after));
    trace.push(WalletTraceStep::new(
        id,
        WalletTraceAction::ReuseTerminal,
        after,
        after,
    ));
    trace
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(128))]

    #[test]
    fn generated_legal_generic_executions_are_accepted(
        pending_observations in 0usize..8,
        failed in any::<bool>(),
    ) {
        let trace = generic_trace(pending_observations, failed);
        prop_assert!(check_wallet_trace(&trace, &WalletCheckerConfig::default()).is_ok());
    }

    #[test]
    fn a_second_dispatch_is_always_rejected(pending_observations in 0usize..8) {
        let mut trace = generic_trace(pending_observations, false);
        trace.truncate(2);
        let paying = state(WalletAttemptStatus::GenericPaying, false);
        trace.push(WalletTraceStep::new(
            WalletAttemptId("generated-generic".to_string()),
            WalletTraceAction::BeginDispatch,
            paying,
            paying,
        ));
        prop_assert!(matches!(
            check_wallet_trace(&trace, &WalletCheckerConfig::default()),
            Err(WalletCheckError::IllegalTransition { step: 2, .. })
        ), "second dispatch was accepted");
    }
}

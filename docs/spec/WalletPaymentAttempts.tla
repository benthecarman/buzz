----------------------- MODULE WalletPaymentAttempts -----------------------
(***************************************************************************)
(* Durable payment-attempt state machine used by the experimental desktop  *)
(* wallet, including approved NWC-321 requests from managed agents. The     *)
(* safety boundary is persistence before provider dispatch:                 *)
(* after status becomes Paying, retries may only reconcile.                 *)
(*                                                                         *)
(* The runtime checker validates a product of these single-attempt          *)
(* machines, keyed by opaque attempt id. Attempts do not interact, so the   *)
(* single-attempt model is sufficient for the local transition invariant.   *)
(***************************************************************************)
EXTENDS Naturals

Statuses == {
    "absent",
    "generic_prepared",
    "generic_paying",
    "generic_completed",
    "generic_failed",
    "profile_prepared",
    "profile_paying",
    "profile_paid_without_proof",
    "profile_failed"
}

TerminalStatuses == {
    "generic_completed",
    "generic_failed",
    "profile_paid_without_proof",
    "profile_failed"
}

VARIABLES status, paymentRecorded, dispatchCount

vars == <<status, paymentRecorded, dispatchCount>>

Init ==
    /\ status = "absent"
    /\ paymentRecorded = FALSE
    /\ dispatchCount = 0

PrepareGeneric ==
    /\ status = "absent"
    /\ status' = "generic_prepared"
    /\ paymentRecorded' = FALSE
    /\ UNCHANGED dispatchCount

PrepareProfile ==
    /\ status = "absent"
    /\ status' = "profile_prepared"
    /\ paymentRecorded' = FALSE
    /\ UNCHANGED dispatchCount

(***************************************************************************)
(* This action occurs after Paying is durably stored and immediately before *)
(* the only provider send call. Its guard makes a second dispatch illegal.   *)
(***************************************************************************)
BeginDispatch ==
    /\ status \in {"generic_prepared", "profile_prepared"}
    /\ status' = IF status = "generic_prepared"
                  THEN "generic_paying"
                  ELSE "profile_paying"
    /\ paymentRecorded' = FALSE
    /\ dispatchCount' = dispatchCount + 1

Reconcile ==
    /\ status \in {"generic_paying", "profile_paying"}
    /\ UNCHANGED vars

RecordPending ==
    /\ status \in {"generic_paying", "profile_paying"}
    /\ paymentRecorded' = TRUE
    /\ UNCHANGED <<status, dispatchCount>>

RecordCompleted ==
    /\ status = "generic_paying"
    /\ status' = "generic_completed"
    /\ paymentRecorded' = TRUE
    /\ UNCHANGED dispatchCount

RecordPaidWithoutProof ==
    /\ status = "profile_paying"
    /\ status' = "profile_paid_without_proof"
    /\ paymentRecorded' = TRUE
    /\ UNCHANGED dispatchCount

RecordFailed(hasPayment) ==
    /\ status \in {"generic_paying", "profile_paying"}
    /\ paymentRecorded => hasPayment
    /\ status' = IF status = "generic_paying"
                  THEN "generic_failed"
                  ELSE "profile_failed"
    /\ paymentRecorded' = hasPayment
    /\ UNCHANGED dispatchCount

ReuseTerminal ==
    /\ status \in TerminalStatuses
    /\ UNCHANGED vars

RejectConflict ==
    /\ status # "absent"
    /\ UNCHANGED vars

Next ==
    \/ PrepareGeneric
    \/ PrepareProfile
    \/ BeginDispatch
    \/ Reconcile
    \/ RecordPending
    \/ RecordCompleted
    \/ RecordPaidWithoutProof
    \/ RecordFailed(TRUE)
    \/ RecordFailed(FALSE)
    \/ ReuseTerminal
    \/ RejectConflict

Spec == Init /\ [][Next]_vars

TypeOK ==
    /\ status \in Statuses
    /\ paymentRecorded \in BOOLEAN
    /\ dispatchCount \in Nat

NoSecondDispatch == dispatchCount <= 1

SuccessfulAttemptHasPayment ==
    status \in {"generic_completed", "profile_paid_without_proof"}
        => paymentRecorded

PreparedAttemptHasNoPayment ==
    status \in {"generic_prepared", "profile_prepared"}
        => ~paymentRecorded

Safety ==
    /\ TypeOK
    /\ NoSecondDispatch
    /\ SuccessfulAttemptHasPayment
    /\ PreparedAttemptHasNoPayment

=============================================================================

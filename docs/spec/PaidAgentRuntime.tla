-------------------------- MODULE PaidAgentRuntime --------------------------
(***************************************************************************)
(* Prepaid, agent-specific runtime credit. Payments become credit only      *)
(* after wallet settlement verification, with no quote preceding them: the  *)
(* payer buys against published terms. The agent authorizes the scope       *)
(* (access mode, non-DM channel, community) before it locks any credit.     *)
(* Reservations lock a cap before an instruction can start. Only time       *)
(* inside an active ACP session/prompt is checkpointed as billable runtime. *)
(***************************************************************************)
EXTENDS Naturals, FiniteSets

CONSTANTS Payments, Reservations, Instructions, Caps, Durations, None

VARIABLES scopeAuthorized, settledPayments, deposits, totalCredit,
          reservationCap, reservationInstruction, reservationDispatched,
          meterState, checkpoint,
          settlement, totalLocked, totalUsed

vars == <<scopeAuthorized, settledPayments, deposits, totalCredit,
          reservationCap, reservationInstruction, reservationDispatched,
          meterState, checkpoint,
          settlement, totalLocked, totalUsed>>

Available == totalCredit - totalUsed - totalLocked

Init ==
    /\ scopeAuthorized = FALSE
    /\ settledPayments = {}
    /\ deposits = [p \in Payments |-> 0]
    /\ totalCredit = 0
    /\ reservationCap = [r \in Reservations |-> 0]
    /\ reservationInstruction = [r \in Reservations |-> None]
    /\ reservationDispatched = [r \in Reservations |-> FALSE]
    /\ meterState = [r \in Reservations |-> "idle"]
    /\ checkpoint = [r \in Reservations |-> 0]
    /\ settlement = [r \in Reservations |-> 0]
    /\ totalLocked = 0
    /\ totalUsed = 0

AuthorizeScope ==
    /\ ~scopeAuthorized
    /\ scopeAuthorized' = TRUE
    /\ UNCHANGED <<settledPayments, deposits, totalCredit, reservationCap,
                    reservationInstruction, reservationDispatched, meterState, checkpoint,
                    settlement, totalLocked, totalUsed>>

SettlePayment(p) ==
    /\ p \in Payments
    /\ settledPayments' = settledPayments \cup {p}
    /\ UNCHANGED <<scopeAuthorized, deposits, totalCredit, reservationCap,
                    reservationInstruction, reservationDispatched, meterState, checkpoint,
                    settlement, totalLocked, totalUsed>>

DepositCredit(p, amount) ==
    /\ p \in settledPayments
    /\ deposits[p] = 0
    /\ amount \in Durations \ {0}
    /\ deposits' = [deposits EXCEPT ![p] = amount]
    /\ totalCredit' = totalCredit + amount
    /\ UNCHANGED <<scopeAuthorized, settledPayments, reservationCap,
                    reservationInstruction, reservationDispatched, meterState, checkpoint,
                    settlement, totalLocked, totalUsed>>

ReserveRuntime(r, cap) ==
    /\ scopeAuthorized
    /\ r \in Reservations
    /\ reservationCap[r] = 0
    /\ cap \in Caps
    /\ cap <= Available
    /\ reservationCap' = [reservationCap EXCEPT ![r] = cap]
    /\ totalLocked' = totalLocked + cap
    /\ UNCHANGED <<scopeAuthorized, settledPayments, deposits, totalCredit,
                    reservationInstruction, reservationDispatched, meterState, checkpoint,
                    settlement, totalUsed>>

BindInstruction(r, i) ==
    /\ r \in Reservations
    /\ i \in Instructions
    /\ reservationCap[r] > 0
    /\ reservationInstruction[r] = None
    /\ reservationInstruction' = [reservationInstruction EXCEPT ![r] = i]
    /\ UNCHANGED <<scopeAuthorized, settledPayments, deposits, totalCredit,
                    reservationCap, reservationDispatched, meterState, checkpoint, settlement,
                    totalLocked, totalUsed>>

DispatchInstruction(r, i) ==
    /\ r \in Reservations
    /\ i \in Instructions
    /\ reservationInstruction[r] = i
    /\ ~reservationDispatched[r]
    /\ reservationDispatched' = [reservationDispatched EXCEPT ![r] = TRUE]
    /\ UNCHANGED <<scopeAuthorized, settledPayments, deposits, totalCredit,
                    reservationCap, reservationInstruction, meterState,
                    checkpoint, settlement, totalLocked, totalUsed>>

StartMeter(r) ==
    /\ reservationDispatched[r]
    /\ meterState[r] \in {"idle", "paused"}
    /\ meterState[r] # "settled"
    /\ checkpoint[r] < reservationCap[r]
    /\ meterState' = [meterState EXCEPT ![r] = "active"]
    /\ UNCHANGED <<scopeAuthorized, settledPayments, deposits, totalCredit,
                    reservationCap, reservationInstruction, reservationDispatched, checkpoint,
                    settlement, totalLocked, totalUsed>>

CheckpointMeter(r, elapsed) ==
    /\ meterState[r] = "active"
    /\ elapsed \in Durations
    /\ checkpoint[r] <= elapsed
    /\ elapsed <= reservationCap[r]
    /\ checkpoint' = [checkpoint EXCEPT ![r] = elapsed]
    /\ UNCHANGED <<scopeAuthorized, settledPayments, deposits, totalCredit,
                    reservationCap, reservationInstruction, reservationDispatched, meterState,
                    settlement, totalLocked, totalUsed>>

PauseMeter(r) ==
    /\ meterState[r] = "active"
    /\ meterState' = [meterState EXCEPT ![r] = "paused"]
    /\ UNCHANGED <<scopeAuthorized, settledPayments, deposits, totalCredit,
                    reservationCap, reservationInstruction, reservationDispatched, checkpoint,
                    settlement, totalLocked, totalUsed>>

SettleReservation(r, used) ==
    /\ reservationCap[r] > 0
    /\ meterState[r] # "settled"
    /\ used \in Durations
    /\ used <= reservationCap[r]
    /\ settlement' = [settlement EXCEPT ![r] = used]
    /\ totalLocked' = totalLocked - reservationCap[r]
    /\ totalUsed' = totalUsed + used
    /\ meterState' = [meterState EXCEPT ![r] = "settled"]
    /\ UNCHANGED <<scopeAuthorized, settledPayments, deposits, totalCredit,
                    reservationCap, reservationInstruction, reservationDispatched, checkpoint>>

BudgetExhausted(r) ==
    /\ meterState[r] = "active"
    /\ checkpoint[r] = reservationCap[r]
    /\ meterState[r] # "settled"
    /\ settlement' = [settlement EXCEPT ![r] = reservationCap[r]]
    /\ totalLocked' = totalLocked - reservationCap[r]
    /\ totalUsed' = totalUsed + reservationCap[r]
    /\ meterState' = [meterState EXCEPT ![r] = "settled"]
    /\ UNCHANGED <<scopeAuthorized, settledPayments, deposits, totalCredit,
                    reservationCap, reservationInstruction, reservationDispatched, checkpoint>>

DuplicateReuse == UNCHANGED vars

Next ==
    \/ AuthorizeScope
    \/ \E p \in Payments : SettlePayment(p)
    \/ \E p \in Payments, amount \in Durations : DepositCredit(p, amount)
    \/ \E r \in Reservations, cap \in Caps : ReserveRuntime(r, cap)
    \/ \E r \in Reservations, i \in Instructions : BindInstruction(r, i)
    \/ \E r \in Reservations, i \in Instructions : DispatchInstruction(r, i)
    \/ \E r \in Reservations : StartMeter(r)
    \/ \E r \in Reservations, elapsed \in Durations : CheckpointMeter(r, elapsed)
    \/ \E r \in Reservations : PauseMeter(r)
    \/ \E r \in Reservations, used \in Durations : SettleReservation(r, used)
    \/ \E r \in Reservations : BudgetExhausted(r)
    \/ DuplicateReuse

Spec == Init /\ [][Next]_vars

TypeOK ==
    /\ settledPayments \subseteq Payments
    /\ totalCredit \in Nat
    /\ totalLocked \in Nat
    /\ totalUsed \in Nat
    /\ Available \in Nat

NoCreditWithoutSettlement == \A p \in Payments : deposits[p] > 0 => p \in settledPayments
NoReservationOverspend == totalLocked + totalUsed <= totalCredit
UsageWithinCap == \A r \in Reservations : meterState[r] = "settled" => settlement[r] <= reservationCap[r]
NoBillingOutsidePrompt == \A r \in Reservations : checkpoint[r] > 0 => reservationDispatched[r]
OneInstructionPerReservation == \A r \in Reservations : reservationInstruction[r] \in Instructions \cup {None}
OneDispatchPerReservation == \A r \in Reservations : reservationDispatched[r] => reservationInstruction[r] # None

Safety ==
    /\ TypeOK
    /\ NoCreditWithoutSettlement
    /\ NoReservationOverspend
    /\ UsageWithinCap
    /\ NoBillingOutsidePrompt
    /\ OneInstructionPerReservation
    /\ OneDispatchPerReservation

=============================================================================

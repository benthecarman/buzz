------------------------ MODULE WalletOfferLifecycle ------------------------
(***************************************************************************)
(* Identity-scoped BOLT12 offer publication. Every operation must use kind  *)
(* 10058, be authored by the advertised identity, attempt every target      *)
(* relay exactly once, and keep active/pending offers unique across          *)
(* identities. Every offer is issued by the active user's wallet, even when *)
(* an agent identity authors the announcement. Relay rejection is an         *)
(* observed outcome, not a protocol violation.                              *)
(***************************************************************************)
EXTENDS FiniteSets

CONSTANTS Identities, Offers, Relays, NoOffer

ASSUME NoOffer \notin Offers

Phases == {"idle", "announcing", "withdrawing"}

VARIABLES phase, activeOffer, pendingOffer, targets, attempted, accepted

vars == <<phase, activeOffer, pendingOffer, targets, attempted, accepted>>

Init ==
    /\ phase = [i \in Identities |-> "idle"]
    /\ activeOffer = [i \in Identities |-> NoOffer]
    /\ pendingOffer = [i \in Identities |-> NoOffer]
    /\ targets = [i \in Identities |-> {}]
    /\ attempted = [i \in Identities |-> {}]
    /\ accepted = [i \in Identities |-> {}]

OfferUnused(i, offer) ==
    \A other \in Identities \ {i}:
        /\ activeOffer[other] # offer
        /\ pendingOffer[other] # offer

BeginAnnouncement(i, offer, relaySet, eventKind, author, issuerIsWalletOwner) ==
    /\ phase[i] = "idle"
    /\ offer \in Offers
    /\ relaySet \in SUBSET Relays
    /\ relaySet # {}
    /\ eventKind = 10058
    /\ author = i
    /\ issuerIsWalletOwner = TRUE
    /\ OfferUnused(i, offer)
    /\ phase' = [phase EXCEPT ![i] = "announcing"]
    /\ pendingOffer' = [pendingOffer EXCEPT ![i] = offer]
    /\ targets' = [targets EXCEPT ![i] = relaySet]
    /\ attempted' = [attempted EXCEPT ![i] = {}]
    /\ accepted' = [accepted EXCEPT ![i] = {}]
    /\ UNCHANGED activeOffer

BeginWithdrawal(i, relaySet, eventKind, author) ==
    /\ phase[i] = "idle"
    /\ relaySet \in SUBSET Relays
    /\ relaySet # {}
    /\ eventKind = 10058
    /\ author = i
    /\ phase' = [phase EXCEPT ![i] = "withdrawing"]
    /\ pendingOffer' = [pendingOffer EXCEPT ![i] = NoOffer]
    /\ targets' = [targets EXCEPT ![i] = relaySet]
    /\ attempted' = [attempted EXCEPT ![i] = {}]
    /\ accepted' = [accepted EXCEPT ![i] = {}]
    /\ UNCHANGED activeOffer

RelayResult(i, relay, wasAccepted) ==
    /\ phase[i] \in {"announcing", "withdrawing"}
    /\ relay \in targets[i]
    /\ relay \notin attempted[i]
    /\ attempted' = [attempted EXCEPT ![i] = @ \cup {relay}]
    /\ accepted' = [accepted EXCEPT
         ![i] = IF wasAccepted THEN @ \cup {relay} ELSE @]
    /\ UNCHANGED <<phase, activeOffer, pendingOffer, targets>>

FinishAnnouncement(i) ==
    /\ phase[i] = "announcing"
    /\ attempted[i] = targets[i]
    /\ phase' = [phase EXCEPT ![i] = "idle"]
    /\ activeOffer' = [activeOffer EXCEPT ![i] = pendingOffer[i]]
    /\ pendingOffer' = [pendingOffer EXCEPT ![i] = NoOffer]
    /\ targets' = [targets EXCEPT ![i] = {}]
    /\ attempted' = [attempted EXCEPT ![i] = {}]
    /\ accepted' = [accepted EXCEPT ![i] = {}]

FinishWithdrawal(i) ==
    /\ phase[i] = "withdrawing"
    /\ attempted[i] = targets[i]
    /\ phase' = [phase EXCEPT ![i] = "idle"]
    /\ activeOffer' = [activeOffer EXCEPT ![i] = NoOffer]
    /\ pendingOffer' = [pendingOffer EXCEPT ![i] = NoOffer]
    /\ targets' = [targets EXCEPT ![i] = {}]
    /\ attempted' = [attempted EXCEPT ![i] = {}]
    /\ accepted' = [accepted EXCEPT ![i] = {}]

Abort(i) ==
    /\ phase[i] \in {"announcing", "withdrawing"}
    /\ phase' = [phase EXCEPT ![i] = "idle"]
    /\ pendingOffer' = [pendingOffer EXCEPT ![i] = NoOffer]
    /\ targets' = [targets EXCEPT ![i] = {}]
    /\ attempted' = [attempted EXCEPT ![i] = {}]
    /\ accepted' = [accepted EXCEPT ![i] = {}]
    /\ UNCHANGED activeOffer

Next ==
    \/ \E i \in Identities, offer \in Offers, relaySet \in SUBSET Relays:
           BeginAnnouncement(i, offer, relaySet, 10058, i, TRUE)
    \/ \E i \in Identities, relaySet \in SUBSET Relays:
           BeginWithdrawal(i, relaySet, 10058, i)
    \/ \E i \in Identities, relay \in Relays, ok \in BOOLEAN:
           RelayResult(i, relay, ok)
    \/ \E i \in Identities: FinishAnnouncement(i)
    \/ \E i \in Identities: FinishWithdrawal(i)
    \/ \E i \in Identities: Abort(i)

Spec == Init /\ [][Next]_vars

TypeOK ==
    /\ phase \in [Identities -> Phases]
    /\ activeOffer \in [Identities -> Offers \cup {NoOffer}]
    /\ pendingOffer \in [Identities -> Offers \cup {NoOffer}]
    /\ targets \in [Identities -> SUBSET Relays]
    /\ attempted \in [Identities -> SUBSET Relays]
    /\ accepted \in [Identities -> SUBSET Relays]

AttemptsAreTargets ==
    \A i \in Identities:
        /\ attempted[i] \subseteq targets[i]
        /\ accepted[i] \subseteq attempted[i]

OffersAreUnique ==
    \A i, other \in Identities:
        (i # other) =>
            /\ (activeOffer[i] = NoOffer \/ activeOffer[i] # activeOffer[other])
            /\ (pendingOffer[i] = NoOffer \/ pendingOffer[i] # pendingOffer[other])
            /\ (activeOffer[i] = NoOffer \/ activeOffer[i] # pendingOffer[other])

IdleIsQuiescent ==
    \A i \in Identities:
        phase[i] = "idle" =>
            /\ pendingOffer[i] = NoOffer
            /\ targets[i] = {}
            /\ attempted[i] = {}
            /\ accepted[i] = {}

Safety == TypeOK /\ AttemptsAreTargets /\ OffersAreUnique /\ IdleIsQuiescent

=============================================================================

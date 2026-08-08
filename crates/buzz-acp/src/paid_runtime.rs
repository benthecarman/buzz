//! Agent-side reservation protocol and ledger replay for paid runtime.

use std::{
    collections::{BTreeMap, HashMap, HashSet, VecDeque},
    fs::OpenOptions,
    io::Write,
    path::{Path, PathBuf},
    str::FromStr,
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc, Mutex, OnceLock,
    },
    time::{Duration, Instant},
};

use buzz_core::agent_runtime_payment::{
    DepositRecord, ReservationRecord, RuntimeDeposit, RuntimeLedger, RuntimeOutcome,
    RuntimePricing, RuntimeQuote, RuntimeReservation, RuntimeReservationRequest,
    RuntimeReservationResponse, RuntimeSettlement, SettlementRecord, VERSION,
};
use buzz_core::kind::{
    KIND_AGENT_RUNTIME_DEPOSIT, KIND_AGENT_RUNTIME_REQUEST, KIND_AGENT_RUNTIME_RESERVATION,
    KIND_AGENT_RUNTIME_RESPONSE, KIND_AGENT_RUNTIME_SETTLEMENT, KIND_BOLT12_OFFER,
};
use lightning::offers::offer::Offer;
use nostr::{
    nips::nip44::{self, Version},
    Alphabet, Event, EventBuilder, EventId, JsonUtil, Kind, PublicKey, SingleLetterTag, Tag,
    Timestamp,
};
use serde::{Deserialize, Serialize};
use tokio::task::JoinHandle;

use crate::{config::Config, relay::RestClient};

const REQUEST_TTL_SECS: u64 = 5 * 60;
const LEDGER_PAGE_SIZE: usize = 250;
const RATE_LIMIT_WINDOW: Duration = Duration::from_secs(60);
const MAX_RESERVATION_REQUESTS_PER_WINDOW: usize = 60;
const MAX_PAID_INVOCATIONS_PER_WINDOW: usize = 30;
const MAX_RATE_LIMIT_SCOPES: usize = 10_000;

#[derive(Default)]
struct SlidingWindowLimiter {
    attempts: HashMap<String, VecDeque<(Instant, String)>>,
}

impl SlidingWindowLimiter {
    fn check(&mut self, scope: &str, idempotency_id: &str, limit: usize, now: Instant) -> bool {
        if self.attempts.len() >= MAX_RATE_LIMIT_SCOPES && !self.attempts.contains_key(scope) {
            self.attempts.retain(|_, attempts| {
                attempts
                    .back()
                    .is_some_and(|(at, _)| now.duration_since(*at) < RATE_LIMIT_WINDOW)
            });
            if self.attempts.len() >= MAX_RATE_LIMIT_SCOPES {
                return false;
            }
        }
        let attempts = self.attempts.entry(scope.to_string()).or_default();
        while attempts
            .front()
            .is_some_and(|(at, _)| now.duration_since(*at) >= RATE_LIMIT_WINDOW)
        {
            attempts.pop_front();
        }
        if attempts.iter().any(|(_, id)| id == idempotency_id) {
            return true;
        }
        if attempts.len() >= limit {
            return false;
        }
        attempts.push_back((now, idempotency_id.to_string()));
        true
    }
}

static RESERVATION_REQUEST_LIMITER: OnceLock<Mutex<SlidingWindowLimiter>> = OnceLock::new();
static PAID_INVOCATION_LIMITER: OnceLock<Mutex<SlidingWindowLimiter>> = OnceLock::new();
static PROCESS_NONCE: OnceLock<String> = OnceLock::new();
static ACTIVE_EXECUTION_LEASES: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();
static ACTIVE_RUNTIME_INSTANCE_LOCK: OnceLock<std::fs::File> = OnceLock::new();

fn protocol_error(message: impl Into<String>) -> anyhow::Error {
    anyhow::anyhow!(message.into())
}

fn exactly_one_tag<'a>(event: &'a Event, name: &str) -> anyhow::Result<&'a str> {
    let values = event
        .tags
        .iter()
        .filter_map(|tag| {
            let parts = tag.as_slice();
            (parts.len() == 2 && parts[0].as_str() == name).then(|| parts[1].as_str())
        })
        .collect::<Vec<_>>();
    match values.as_slice() {
        [value] => Ok(value),
        _ => Err(protocol_error(format!("expected exactly one {name} tag"))),
    }
}

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

/// Operational kill switch. Existing prompt meters continue to settle, while
/// new reservations and external paid admission fail closed.
pub fn kill_switch_active() -> bool {
    std::env::var("BUZZ_ACP_DISABLE_PAID_RUNTIME")
        .ok()
        .is_some_and(|value| {
            matches!(
                value.trim().to_ascii_lowercase().as_str(),
                "1" | "true" | "yes" | "on"
            )
        })
}

/// Require a live scoped BOLT12 offer before advertising paid runtime.
pub async fn validate_pricing_readiness(rest: &RestClient, agent: PublicKey) -> anyhow::Result<()> {
    latest_offer(rest, agent).await.map(|_| ())
}

/// Hold one OS-level writer fence for this agent's durable runtime state.
pub fn acquire_runtime_instance_lock(keys: &nostr::Keys) -> anyhow::Result<()> {
    if ACTIVE_RUNTIME_INSTANCE_LOCK.get().is_some() {
        return Ok(());
    }
    let directory = runtime_directory(keys);
    create_private_directory(&directory)?;
    let path = directory.join("active-instance.lock");
    let mut options = OpenOptions::new();
    options.read(true).write(true).create(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let file = options.open(path)?;
    fs2::FileExt::try_lock_exclusive(&file).map_err(|error| {
        protocol_error(format!(
            "another paid-runtime harness owns this agent state directory: {error}"
        ))
    })?;
    ACTIVE_RUNTIME_INSTANCE_LOCK
        .set(file)
        .map_err(|_| protocol_error("paid-runtime instance lock raced during startup"))?;
    Ok(())
}

fn check_rate_limit(
    limiter: &'static OnceLock<Mutex<SlidingWindowLimiter>>,
    scope: &str,
    idempotency_id: &str,
    limit: usize,
) -> anyhow::Result<()> {
    let mut limiter = limiter
        .get_or_init(|| Mutex::new(SlidingWindowLimiter::default()))
        .lock()
        .map_err(|_| protocol_error("paid runtime rate limiter is unavailable"))?;
    if limiter.check(scope, idempotency_id, limit, Instant::now()) {
        Ok(())
    } else {
        Err(protocol_error("paid runtime is temporarily unavailable"))
    }
}

fn event_from_value(value: &serde_json::Value) -> anyhow::Result<Event> {
    Event::from_json(value.to_string()).map_err(|error| protocol_error(error.to_string()))
}

async fn query_events(rest: &RestClient, filter: nostr::Filter) -> anyhow::Result<Vec<Event>> {
    let mut cursor = None;
    let mut seen = HashSet::<EventId>::new();
    let mut events = Vec::new();
    loop {
        let mut page_filter = filter.clone().limit(LEDGER_PAGE_SIZE);
        if let Some(until) = cursor {
            page_filter = page_filter.until(Timestamp::from_secs(until));
        }
        let value = rest.query(&[page_filter]).await?;
        let rows = value
            .as_array()
            .ok_or_else(|| protocol_error("relay query did not return an event array"))?;
        let mut oldest = None::<u64>;
        for row in rows {
            let event = event_from_value(row)?;
            oldest = Some(oldest.map_or(event.created_at.as_secs(), |value| {
                value.min(event.created_at.as_secs())
            }));
            if seen.insert(event.id) {
                events.push(event);
            }
        }
        if rows.len() < LEDGER_PAGE_SIZE {
            break;
        }
        let next = oldest.ok_or_else(|| protocol_error("relay returned an empty full page"))?;
        if cursor.is_some_and(|current| next >= current) {
            return Err(protocol_error(
                "runtime ledger pagination did not advance; refusing paid admission",
            ));
        }
        cursor = Some(next);
    }
    Ok(events)
}

fn decrypt_agent_event<T: serde::de::DeserializeOwned>(
    keys: &nostr::Keys,
    event: &Event,
    payer: &PublicKey,
) -> anyhow::Result<T> {
    let plaintext = nip44::decrypt(keys.secret_key(), payer, &event.content)?;
    serde_json::from_str(&plaintext).map_err(Into::into)
}

async fn channel_allows_purchase(
    rest: &RestClient,
    channel_id: &str,
    payer_hex: &str,
    agent_hex: &str,
) -> anyhow::Result<bool> {
    let d_tag = SingleLetterTag::lowercase(Alphabet::D);
    let filters = [
        nostr::Filter::new()
            .kind(Kind::Custom(
                buzz_core::kind::KIND_NIP29_GROUP_METADATA as u16,
            ))
            .custom_tags(d_tag, [channel_id]),
        nostr::Filter::new()
            .kind(Kind::Custom(
                buzz_core::kind::KIND_NIP29_GROUP_MEMBERS as u16,
            ))
            .custom_tags(d_tag, [channel_id]),
    ];
    let value = rest.query(&filters).await?;
    let rows = value
        .as_array()
        .ok_or_else(|| protocol_error("channel validation query was incomplete"))?;
    let mut metadata: Option<Event> = None;
    let mut membership: Option<Event> = None;
    for row in rows {
        let event = event_from_value(row)?;
        match event.kind.as_u16() as u32 {
            buzz_core::kind::KIND_NIP29_GROUP_METADATA
                if metadata.as_ref().is_none_or(|current| {
                    (event.created_at, event.id) > (current.created_at, current.id)
                }) =>
            {
                metadata = Some(event);
            }
            buzz_core::kind::KIND_NIP29_GROUP_MEMBERS
                if membership.as_ref().is_none_or(|current| {
                    (event.created_at, event.id) > (current.created_at, current.id)
                }) =>
            {
                membership = Some(event);
            }
            _ => {}
        }
    }
    let Some(metadata) = metadata else {
        return Ok(false);
    };
    let is_dm = metadata.tags.iter().any(|tag| {
        let parts = tag.as_slice();
        parts.len() >= 2
            && matches!(parts[0].as_str(), "t" | "type")
            && parts[1].eq_ignore_ascii_case("dm")
    });
    let Some(membership) = membership else {
        return Ok(false);
    };
    let members = membership
        .tags
        .iter()
        .filter_map(|tag| {
            let parts = tag.as_slice();
            (parts.len() >= 2 && parts[0].as_str() == "p").then(|| parts[1].as_str())
        })
        .collect::<std::collections::HashSet<_>>();
    Ok(!is_dm && members.contains(payer_hex) && members.contains(agent_hex))
}

async fn latest_offer(rest: &RestClient, agent: PublicKey) -> anyhow::Result<Event> {
    let mut events = query_events(
        rest,
        nostr::Filter::new()
            .author(agent)
            .kind(Kind::Custom(KIND_BOLT12_OFFER as u16)),
    )
    .await?;
    events.sort_by_key(|event| (event.created_at, event.id));
    let event = events
        .pop()
        .ok_or_else(|| protocol_error("agent has no active BOLT12 offer"))?;
    let offers = event
        .tags
        .iter()
        .filter_map(|tag| {
            let parts = tag.as_slice();
            (parts.len() == 2 && parts[0].as_str() == "offer").then(|| parts[1].as_str())
        })
        .collect::<Vec<_>>();
    let parsed_offer = offers
        .first()
        .filter(|_| offers.len() == 1)
        .and_then(|offer| Offer::from_str(offer).ok());
    if parsed_offer
        .as_ref()
        .is_none_or(|offer| offer.to_string() != offers[0])
    {
        return Err(protocol_error(
            "agent BOLT12 offer is withdrawn or malformed",
        ));
    }
    Ok(event)
}

async fn replay_ledger(
    keys: &nostr::Keys,
    rest: &RestClient,
    payer: &PublicKey,
    channel_id: &str,
) -> anyhow::Result<(
    RuntimeLedger,
    BTreeMap<String, (Event, RuntimeReservation)>,
    Vec<(Event, RuntimeReservation)>,
)> {
    let p_tag = SingleLetterTag::lowercase(Alphabet::P);
    let h_tag = SingleLetterTag::lowercase(Alphabet::H);
    let agent = keys.public_key();
    let mut events = query_events(
        rest,
        nostr::Filter::new()
            .author(agent)
            .kinds([
                Kind::Custom(KIND_AGENT_RUNTIME_DEPOSIT as u16),
                Kind::Custom(KIND_AGENT_RUNTIME_RESERVATION as u16),
                Kind::Custom(KIND_AGENT_RUNTIME_SETTLEMENT as u16),
            ])
            .custom_tags(p_tag, [payer.to_hex()])
            .custom_tags(h_tag, [channel_id]),
    )
    .await?;
    // Nostr timestamps have one-second precision. Preserve deterministic
    // `(created_at, event_id)` order within each dependency phase while
    // ensuring a same-second deposit precedes its reservation and a
    // reservation precedes its settlement.
    events.sort_by_key(|event| {
        let phase = match event.kind.as_u16() as u32 {
            KIND_AGENT_RUNTIME_DEPOSIT => 0u8,
            KIND_AGENT_RUNTIME_RESERVATION => 1,
            KIND_AGENT_RUNTIME_SETTLEMENT => 2,
            _ => 3,
        };
        (event.created_at, phase, event.id)
    });
    let mut ledger = RuntimeLedger::default();
    let mut reservations = BTreeMap::<String, (Event, RuntimeReservation)>::new();
    let mut all_reservations = Vec::new();
    let trace_scope = crate::runtime_conformance::scope_id(
        &keys.public_key().to_hex(),
        &payer.to_hex(),
        channel_id,
    );
    let trace_replay = crate::runtime_conformance::begin_scope(trace_scope.clone());
    for event in events {
        event.verify()?;
        if event.pubkey != agent
            || exactly_one_tag(&event, "p")? != payer.to_hex()
            || exactly_one_tag(&event, "h")? != channel_id
        {
            return Err(protocol_error(
                "runtime ledger event does not match its scoped author, payer, or channel",
            ));
        }
        match event.kind.as_u16() as u32 {
            KIND_AGENT_RUNTIME_DEPOSIT => {
                let deposit: RuntimeDeposit = serde_json::from_str(&event.content)?;
                deposit.validate()?;
                let payment_id = exactly_one_tag(&event, "zap_intent")?.to_string();
                EventId::from_hex(exactly_one_tag(&event, "quote")?)?;
                EventId::from_hex(exactly_one_tag(&event, "zap")?)?;
                EventId::from_hex(&payment_id)?;
                ledger.apply_deposit(DepositRecord {
                    payment_id: payment_id.clone(),
                    credit_ms: deposit.credit_ms,
                })?;
                if trace_replay {
                    let opaque_payment =
                        crate::runtime_conformance::entity_id("payment", &payment_id);
                    crate::runtime_conformance::record(
                        trace_scope.clone(),
                        buzz_conformance::paid_agent_runtime::RuntimeTraceAction::PaymentSettled {
                            payment_id: opaque_payment.clone(),
                            verified: true,
                        },
                    );
                    crate::runtime_conformance::record(
                        trace_scope.clone(),
                        buzz_conformance::paid_agent_runtime::RuntimeTraceAction::CreditDeposited {
                            payment_id: opaque_payment,
                            credit_ms: deposit.credit_ms,
                        },
                    );
                }
            }
            KIND_AGENT_RUNTIME_RESERVATION => {
                let reservation: RuntimeReservation = decrypt_agent_event(keys, &event, payer)?;
                reservation.validate()?;
                let expiration = exactly_one_tag(&event, "expiration")?
                    .parse::<u64>()
                    .map_err(|_| protocol_error("runtime reservation expiration is invalid"))?;
                if exactly_one_tag(&event, "encryption")? != "nip44_v2"
                    || expiration != reservation.must_start_by
                    || event.created_at.as_secs() > expiration
                    || expiration > event.created_at.as_secs().saturating_add(REQUEST_TTL_SECS)
                {
                    return Err(protocol_error(
                        "runtime reservation validity does not match its signed tags",
                    ));
                }
                ledger.apply_reservation(ReservationRecord {
                    reservation_id: event.id.to_hex(),
                    cap_ms: reservation.cap_ms,
                })?;
                if trace_replay {
                    crate::runtime_conformance::record(
                        trace_scope.clone(),
                        buzz_conformance::paid_agent_runtime::RuntimeTraceAction::RuntimeReserved {
                            reservation_id: crate::runtime_conformance::entity_id(
                                "reservation",
                                &event.id.to_hex(),
                            ),
                            cap_ms: reservation.cap_ms,
                        },
                    );
                }
                if let Some((existing_event, existing)) = reservations.get(&reservation.request_id)
                {
                    if existing_event.id != event.id || existing != &reservation {
                        return Err(protocol_error(
                            "request identifier produced conflicting reservations",
                        ));
                    }
                } else {
                    reservations.insert(
                        reservation.request_id.clone(),
                        (event.clone(), reservation.clone()),
                    );
                }
                all_reservations.push((event, reservation));
            }
            KIND_AGENT_RUNTIME_SETTLEMENT => {
                let settlement: RuntimeSettlement = decrypt_agent_event(keys, &event, payer)?;
                settlement.validate()?;
                if exactly_one_tag(&event, "encryption")? != "nip44_v2"
                    || exactly_one_tag(&event, "e")? != settlement.reservation_id
                    || ledger.reservation_cap_ms(&settlement.reservation_id)
                        != Some(settlement.cap_ms)
                {
                    return Err(protocol_error(
                        "runtime settlement does not match its reserved cap and routing",
                    ));
                }
                ledger.apply_settlement(SettlementRecord {
                    reservation_id: settlement.reservation_id.clone(),
                    used_ms: settlement.used_ms,
                })?;
                if trace_replay {
                    let reservation_id = crate::runtime_conformance::entity_id(
                        "reservation",
                        &settlement.reservation_id,
                    );
                    if settlement.used_ms > 0 {
                        let bootstrap_instruction = crate::runtime_conformance::entity_id(
                            "bootstrap-instruction",
                            &settlement.reservation_id,
                        );
                        crate::runtime_conformance::record(
                            trace_scope.clone(),
                            buzz_conformance::paid_agent_runtime::RuntimeTraceAction::InstructionBound {
                                reservation_id: reservation_id.clone(),
                                instruction_id: bootstrap_instruction.clone(),
                                allowlisted: true,
                                non_dm: true,
                                same_community: true,
                            },
                        );
                        crate::runtime_conformance::record(
                            trace_scope.clone(),
                            buzz_conformance::paid_agent_runtime::RuntimeTraceAction::InvocationDispatched {
                                reservation_id: reservation_id.clone(),
                                instruction_id: bootstrap_instruction,
                            },
                        );
                        crate::runtime_conformance::record(
                            trace_scope.clone(),
                            buzz_conformance::paid_agent_runtime::RuntimeTraceAction::MeterStarted {
                                reservation_id: reservation_id.clone(),
                            },
                        );
                        crate::runtime_conformance::record(
                            trace_scope.clone(),
                            buzz_conformance::paid_agent_runtime::RuntimeTraceAction::MeterCheckpointed {
                                reservation_id: reservation_id.clone(),
                                elapsed_ms: settlement.used_ms,
                            },
                        );
                    }
                    crate::runtime_conformance::record(
                        trace_scope.clone(),
                        buzz_conformance::paid_agent_runtime::RuntimeTraceAction::ReservationSettled {
                            reservation_id,
                            used_ms: settlement.used_ms,
                            outcome: trace_outcome(settlement.outcome),
                        },
                    );
                }
            }
            _ => return Err(protocol_error("unexpected runtime ledger event kind")),
        }
    }
    Ok((ledger, reservations, all_reservations))
}

fn trace_outcome(
    outcome: RuntimeOutcome,
) -> buzz_conformance::paid_agent_runtime::RuntimeTraceOutcome {
    use buzz_conformance::paid_agent_runtime::RuntimeTraceOutcome as Trace;
    match outcome {
        RuntimeOutcome::Completed => Trace::Completed,
        RuntimeOutcome::Error => Trace::Error,
        RuntimeOutcome::Cancelled => Trace::Cancelled,
        RuntimeOutcome::Timeout => Trace::Timeout,
        RuntimeOutcome::BudgetExhausted => Trace::BudgetExhausted,
        RuntimeOutcome::Interrupted => Trace::Interrupted,
        RuntimeOutcome::UnusedExpired => Trace::UnusedExpired,
    }
}

fn signed_encrypted_response(
    config: &Config,
    payer: &PublicKey,
    response: &RuntimeReservationResponse,
    expires_at: u64,
) -> anyhow::Result<Event> {
    response.validate()?;
    let plaintext = serde_json::to_string(response)?;
    let ciphertext = nip44::encrypt(config.keys.secret_key(), payer, plaintext, Version::V2)?;
    Ok(
        EventBuilder::new(Kind::Custom(KIND_AGENT_RUNTIME_RESPONSE as u16), ciphertext)
            .tags([
                Tag::parse(["p", payer.to_hex().as_str()])?,
                Tag::parse(["expiration", expires_at.to_string().as_str()])?,
                Tag::parse(["encryption", "nip44_v2"])?,
            ])
            .sign_with_keys(&config.keys)?,
    )
}

fn reservation_path(config: &Config, request_id: &str) -> PathBuf {
    runtime_directory(&config.keys).join(format!("reservation-{}.json", hex::encode(request_id)))
}

fn persist_reservation(config: &Config, request_id: &str, event: &Event) -> anyhow::Result<()> {
    let path = reservation_path(config, request_id);
    if create_once(&path, event.as_json().as_bytes())? {
        return Ok(());
    }
    match load_json::<Event>(&path)? {
        Some(existing) if existing.id == event.id && existing == *event => Ok(()),
        Some(_) => Err(protocol_error(
            "request identifier already has a conflicting durable reservation",
        )),
        None => Err(protocol_error("durable runtime reservation disappeared")),
    }
}

fn validate_persisted_reservation(
    config: &Config,
    payer: &PublicKey,
    request: &RuntimeReservationRequest,
    event: &Event,
) -> anyhow::Result<()> {
    event.verify()?;
    let reservation: RuntimeReservation = decrypt_agent_event(&config.keys, event, payer)?;
    let expected_cap = u64::from(request.cap_minutes) * 60_000;
    if event.pubkey != config.keys.public_key()
        || event.kind.as_u16() as u32 != KIND_AGENT_RUNTIME_RESERVATION
        || exactly_one_tag(event, "p")? != payer.to_hex()
        || exactly_one_tag(event, "h")? != request.channel_id
        || reservation.request_id != request.request_id
        || reservation.cap_ms != expected_cap
    {
        return Err(protocol_error(
            "durable reservation does not match the retry request",
        ));
    }
    Ok(())
}

async fn settle_expired_scope_reservations(
    keys: &nostr::Keys,
    rest: &RestClient,
    payer: &PublicKey,
    channel_id: &str,
    ledger: &RuntimeLedger,
    reservations: &[(Event, RuntimeReservation)],
) -> anyhow::Result<bool> {
    let mut settled_any = false;
    let now = now_secs();
    for (event, reservation) in reservations {
        let reservation_id = event.id.to_hex();
        if reservation.must_start_by >= now || !ledger.reservation_is_open(&reservation_id) {
            continue;
        }
        let binding = BoundReservation {
            reservation_id,
            request_id: reservation.request_id.clone(),
            instruction_event_id: String::new(),
            payer_pubkey: payer.to_hex(),
            channel_id: channel_id.to_string(),
            cap_ms: reservation.cap_ms,
        };
        publish_settlement(
            keys,
            rest,
            &binding,
            0,
            RuntimeOutcome::UnusedExpired,
            false,
        )
        .await?;
        settled_any = true;
    }
    Ok(settled_any)
}

/// Replay every payer/channel scope with an authored reservation and close
/// expired unconsumed locks. This runs even when pricing is currently off so
/// retained balances cannot remain stranded behind an abandoned checkout.
pub async fn sweep_expired_open_reservations(
    keys: &nostr::Keys,
    rest: &RestClient,
) -> anyhow::Result<u64> {
    let agent = keys.public_key();
    let events = query_events(
        rest,
        nostr::Filter::new()
            .author(agent)
            .kind(Kind::Custom(KIND_AGENT_RUNTIME_RESERVATION as u16)),
    )
    .await?;
    let mut scopes = BTreeMap::<(String, String), PublicKey>::new();
    for event in events {
        event.verify()?;
        if event.pubkey != agent {
            return Err(protocol_error("reservation sweep returned another author"));
        }
        let payer_hex = exactly_one_tag(&event, "p")?.to_string();
        let channel_id = exactly_one_tag(&event, "h")?.to_string();
        let payer = PublicKey::from_hex(&payer_hex)?;
        let reservation: RuntimeReservation = decrypt_agent_event(keys, &event, &payer)?;
        reservation.validate()?;
        scopes.insert((payer_hex, channel_id), payer);
    }
    let mut settled = 0u64;
    for ((_, channel_id), payer) in scopes {
        let (ledger, _, reservations) = replay_ledger(keys, rest, &payer, &channel_id).await?;
        let before = reservations
            .iter()
            .filter(|(event, reservation)| {
                reservation.must_start_by < now_secs()
                    && ledger.reservation_is_open(&event.id.to_hex())
            })
            .count() as u64;
        if before > 0
            && settle_expired_scope_reservations(
                keys,
                rest,
                &payer,
                &channel_id,
                &ledger,
                &reservations,
            )
            .await?
        {
            settled = settled.saturating_add(before);
        }
    }
    Ok(settled)
}

async fn process_request(
    config: &Config,
    rest: &RestClient,
    payer: &PublicKey,
    request: &RuntimeReservationRequest,
) -> anyhow::Result<RuntimeReservationResponse> {
    request.validate()?;
    if kill_switch_active() {
        return Err(protocol_error("paid runtime is disabled by the operator"));
    }
    let payer_hex = payer.to_hex();
    check_rate_limit(
        &RESERVATION_REQUEST_LIMITER,
        &format!("{}:{}", payer_hex, request.channel_id),
        &request.request_id,
        MAX_RESERVATION_REQUESTS_PER_WINDOW,
    )?;
    if !config.respond_to_allowlist.contains(&payer_hex) {
        return Err(protocol_error("payer is unavailable"));
    }
    let rate = config
        .price_per_minute_sats
        .ok_or_else(|| protocol_error("runtime pricing is unavailable"))?;
    let pricing = RuntimePricing::enabled(rate)?;
    pricing.validate()?;
    if !channel_allows_purchase(
        rest,
        &request.channel_id,
        &payer_hex,
        &config.keys.public_key().to_hex(),
    )
    .await?
    {
        return Err(protocol_error("purchase channel is unavailable"));
    }
    let offer_event = latest_offer(rest, config.keys.public_key()).await?;
    if let Some(persisted) = load_json::<Event>(&reservation_path(config, &request.request_id))? {
        validate_persisted_reservation(config, payer, request, &persisted)?;
        rest.submit_event(&persisted).await?;
    }
    let (mut ledger, mut reservations, all_reservations) =
        replay_ledger(&config.keys, rest, payer, &request.channel_id).await?;
    if settle_expired_scope_reservations(
        &config.keys,
        rest,
        payer,
        &request.channel_id,
        &ledger,
        &all_reservations,
    )
    .await?
    {
        (ledger, reservations, _) =
            replay_ledger(&config.keys, rest, payer, &request.channel_id).await?;
    }
    if let Some((event, existing)) = reservations.get(&request.request_id) {
        let expected_cap = u64::from(request.cap_minutes) * 60_000;
        if existing.cap_ms != expected_cap {
            return Err(protocol_error(
                "request identifier conflicts with an existing cap",
            ));
        }
        if !ledger.reservation_is_open(&event.id.to_hex()) {
            return Err(protocol_error(
                "request identifier belongs to a closed reservation",
            ));
        }
        crate::runtime_conformance::record(
            crate::runtime_conformance::scope_id(
                &config.keys.public_key().to_hex(),
                &payer_hex,
                &request.channel_id,
            ),
            buzz_conformance::paid_agent_runtime::RuntimeTraceAction::DuplicateReused {
                entity_id: crate::runtime_conformance::entity_id("reservation", &event.id.to_hex()),
            },
        );
        return Ok(RuntimeReservationResponse::Reserved {
            version: VERSION,
            request_id: request.request_id.clone(),
            reservation_event: serde_json::to_value(event)?,
        });
    }

    let cap_ms = u64::from(request.cap_minutes) * 60_000;
    if ledger.available_ms()? >= cap_ms {
        let expires_at = now_secs().saturating_add(REQUEST_TTL_SECS);
        let reservation = RuntimeReservation {
            version: VERSION,
            request_id: request.request_id.clone(),
            cap_ms,
            must_start_by: expires_at,
        };
        reservation.validate()?;
        let plaintext = serde_json::to_string(&reservation)?;
        let ciphertext = nip44::encrypt(config.keys.secret_key(), payer, plaintext, Version::V2)?;
        let event = EventBuilder::new(
            Kind::Custom(KIND_AGENT_RUNTIME_RESERVATION as u16),
            ciphertext,
        )
        .tags([
            Tag::parse(["p", payer_hex.as_str()])?,
            Tag::parse(["h", request.channel_id.as_str()])?,
            Tag::parse(["expiration", expires_at.to_string().as_str()])?,
            Tag::parse(["encryption", "nip44_v2"])?,
        ])
        .sign_with_keys(&config.keys)?;
        ledger.apply_reservation(ReservationRecord {
            reservation_id: event.id.to_hex(),
            cap_ms,
        })?;
        persist_reservation(config, &request.request_id, &event)?;
        crate::runtime_conformance::record(
            crate::runtime_conformance::scope_id(
                &config.keys.public_key().to_hex(),
                &payer_hex,
                &request.channel_id,
            ),
            buzz_conformance::paid_agent_runtime::RuntimeTraceAction::RuntimeReserved {
                reservation_id: crate::runtime_conformance::entity_id(
                    "reservation",
                    &event.id.to_hex(),
                ),
                cap_ms,
            },
        );
        rest.submit_event(&event).await?;
        return Ok(RuntimeReservationResponse::Reserved {
            version: VERSION,
            request_id: request.request_id.clone(),
            reservation_event: serde_json::to_value(event)?,
        });
    }

    let expires_at = now_secs().saturating_add(REQUEST_TTL_SECS);
    let amount_sats = rate
        .checked_mul(u64::from(request.cap_minutes))
        .ok_or_else(|| protocol_error("runtime quote amount overflow"))?;
    crate::runtime_conformance::record(
        crate::runtime_conformance::scope_id(
            &config.keys.public_key().to_hex(),
            &payer_hex,
            &request.channel_id,
        ),
        buzz_conformance::paid_agent_runtime::RuntimeTraceAction::QuoteRequested {
            allowlisted: true,
            non_dm: true,
            same_community: true,
        },
    );
    Ok(RuntimeReservationResponse::PaymentRequired {
        quote: RuntimeQuote {
            version: VERSION,
            request_id: request.request_id.clone(),
            agent_pubkey: config.keys.public_key().to_hex(),
            payer_pubkey: payer_hex,
            channel_id: request.channel_id.clone(),
            cap_minutes: request.cap_minutes,
            pack_minutes: request.cap_minutes,
            price_per_minute_sats: rate,
            amount_sats,
            offer_event: serde_json::to_value(offer_event)?,
            expires_at,
        },
    })
}

/// Validate an encrypted request and publish a generic unavailable, quote, or reservation response.
pub async fn handle_request(config: &Config, rest: &RestClient, event: Event) {
    if event.kind.as_u16() as u32 != KIND_AGENT_RUNTIME_REQUEST
        || event.verify().is_err()
        || exactly_one_tag(&event, "p").ok() != Some(config.keys.public_key().to_hex().as_str())
    {
        return;
    }
    let payer = event.pubkey;
    let expiration = exactly_one_tag(&event, "expiration")
        .ok()
        .and_then(|value| value.parse::<u64>().ok());
    if expiration.is_none_or(|value| value < now_secs() || value > now_secs() + REQUEST_TTL_SECS) {
        return;
    }
    let request = nip44::decrypt(config.keys.secret_key(), &payer, &event.content)
        .ok()
        .and_then(|plaintext| serde_json::from_str::<RuntimeReservationRequest>(&plaintext).ok());
    let Some(request) = request else {
        return;
    };
    let response_expires = now_secs().saturating_add(REQUEST_TTL_SECS);
    let response = match process_request(config, rest, &payer, &request).await {
        Ok(response) => response,
        Err(error) => {
            tracing::warn!(error = %error, "paid runtime reservation unavailable");
            crate::runtime_conformance::record(
                crate::runtime_conformance::scope_id(
                    &config.keys.public_key().to_hex(),
                    &payer.to_hex(),
                    &request.channel_id,
                ),
                buzz_conformance::paid_agent_runtime::RuntimeTraceAction::InvocationRejected,
            );
            RuntimeReservationResponse::Unavailable {
                version: VERSION,
                request_id: request.request_id.clone(),
            }
        }
    };
    match signed_encrypted_response(config, &payer, &response, response_expires) {
        Ok(response_event) => {
            if let Err(error) = rest.submit_event(&response_event).await {
                tracing::warn!(error = %error, "publish paid runtime response");
            }
        }
        Err(error) => tracing::warn!(error = %error, "sign paid runtime response"),
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct BoundReservation {
    reservation_id: String,
    request_id: String,
    instruction_event_id: String,
    payer_pubkey: String,
    channel_id: String,
    cap_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct RuntimeCheckpoint {
    binding: BoundReservation,
    checkpointed_used_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct ExecutionLease {
    process_nonce: String,
    instruction_event_id: String,
}

fn runtime_directory(keys: &nostr::Keys) -> PathBuf {
    std::env::var_os("BUZZ_ACP_RUNTIME_STATE_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            std::env::current_dir()
                .unwrap_or_else(|_| PathBuf::from("."))
                .join(".buzz-agent-runtime")
        })
        .join(keys.public_key().to_hex())
}

fn binding_path(keys: &nostr::Keys, reservation_id: &str) -> PathBuf {
    runtime_directory(keys).join(format!("binding-{reservation_id}.json"))
}

fn checkpoint_path(keys: &nostr::Keys, reservation_id: &str) -> PathBuf {
    runtime_directory(keys).join(format!("checkpoint-{reservation_id}.json"))
}

fn settlement_path(keys: &nostr::Keys, reservation_id: &str) -> PathBuf {
    runtime_directory(keys).join(format!("settlement-{reservation_id}.json"))
}

fn settlement_ack_path(keys: &nostr::Keys, reservation_id: &str) -> PathBuf {
    runtime_directory(keys).join(format!("settlement-{reservation_id}.accepted"))
}

fn execution_lease_path(keys: &nostr::Keys, reservation_id: &str) -> PathBuf {
    runtime_directory(keys).join(format!("execution-{reservation_id}.json"))
}

fn atomic_write(path: &Path, contents: &[u8]) -> anyhow::Result<()> {
    let directory = path
        .parent()
        .ok_or_else(|| protocol_error("runtime state path has no parent"))?;
    create_private_directory(directory)?;
    let temporary = path.with_extension(format!("{}.tmp", uuid::Uuid::new_v4()));
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(&temporary)?;
    file.write_all(contents)?;
    file.sync_all()?;
    std::fs::rename(temporary, path)?;
    sync_directory(directory)?;
    Ok(())
}

fn create_once(path: &Path, contents: &[u8]) -> anyhow::Result<bool> {
    let directory = path
        .parent()
        .ok_or_else(|| protocol_error("runtime state path has no parent"))?;
    create_private_directory(directory)?;
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    match options.open(path) {
        Ok(mut file) => {
            file.write_all(contents)?;
            file.sync_all()?;
            sync_directory(directory)?;
            Ok(true)
        }
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => Ok(false),
        Err(error) => Err(error.into()),
    }
}

fn create_private_directory(path: &Path) -> anyhow::Result<()> {
    std::fs::create_dir_all(path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

fn sync_directory(path: &Path) -> anyhow::Result<()> {
    #[cfg(unix)]
    {
        let directory = std::fs::File::open(path)?;
        directory.sync_all()?;
    }
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}

fn load_json<T: serde::de::DeserializeOwned>(path: &Path) -> anyhow::Result<Option<T>> {
    match std::fs::read(path) {
        Ok(bytes) => Ok(Some(serde_json::from_slice(&bytes)?)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.into()),
    }
}

fn runtime_tag_reservation_optional(
    event: &Event,
    agent_hex: &str,
) -> anyhow::Result<Option<String>> {
    let mut values = Vec::new();
    for tag in event.tags.iter() {
        let parts = tag.as_slice();
        if parts
            .first()
            .is_none_or(|value| value.as_str() != "agent_runtime")
        {
            continue;
        }
        if parts.len() != 3 {
            return Err(protocol_error("malformed agent_runtime tag"));
        }
        if parts[1].as_str() == agent_hex {
            values.push(parts[2].as_str().to_string());
        }
    }
    match values.as_slice() {
        [] => Ok(None),
        [reservation_id] => Ok(Some(reservation_id.clone())),
        _ => Err(protocol_error("multiple matching agent_runtime tags")),
    }
}

fn runtime_tag_reservation(event: &Event, agent_hex: &str) -> anyhow::Result<String> {
    runtime_tag_reservation_optional(event, agent_hex)?.ok_or_else(|| {
        protocol_error("paid instruction requires exactly one matching agent_runtime tag")
    })
}

fn persist_binding_once(keys: &nostr::Keys, binding: &BoundReservation) -> anyhow::Result<bool> {
    let path = binding_path(keys, &binding.reservation_id);
    if create_once(&path, &serde_json::to_vec(binding)?)? {
        return Ok(true);
    }
    match load_json::<BoundReservation>(&path)? {
        Some(existing) if existing == *binding => Ok(false),
        Some(_) => Err(protocol_error(
            "runtime reservation is already bound to another instruction",
        )),
        None => Err(protocol_error("runtime reservation binding disappeared")),
    }
}

fn process_nonce() -> &'static str {
    PROCESS_NONCE
        .get_or_init(|| uuid::Uuid::new_v4().to_string())
        .as_str()
}

fn acquire_execution_lease(keys: &nostr::Keys, binding: &BoundReservation) -> anyhow::Result<()> {
    let lease = ExecutionLease {
        process_nonce: process_nonce().to_string(),
        instruction_event_id: binding.instruction_event_id.clone(),
    };
    let path = execution_lease_path(keys, &binding.reservation_id);
    if !create_once(&path, &serde_json::to_vec(&lease)?)? {
        let existing = load_json::<ExecutionLease>(&path)?
            .ok_or_else(|| protocol_error("runtime execution lease disappeared"))?;
        if existing != lease {
            return Err(protocol_error(
                "runtime reservation is leased by another harness process",
            ));
        }
    }
    let mut active = ACTIVE_EXECUTION_LEASES
        .get_or_init(|| Mutex::new(HashSet::new()))
        .lock()
        .map_err(|_| protocol_error("runtime execution lease state is unavailable"))?;
    if !active.insert(binding.reservation_id.clone()) {
        return Err(protocol_error(
            "runtime reservation is already executing in this harness",
        ));
    }
    Ok(())
}

fn release_execution_lease(keys: &nostr::Keys, reservation_id: &str, completed: bool) {
    if let Ok(mut active) = ACTIVE_EXECUTION_LEASES
        .get_or_init(|| Mutex::new(HashSet::new()))
        .lock()
    {
        active.remove(reservation_id);
    }
    if completed {
        let path = execution_lease_path(keys, reservation_id);
        if let Err(error) = std::fs::remove_file(path) {
            if error.kind() != std::io::ErrorKind::NotFound {
                tracing::warn!(%error, "remove completed runtime execution lease");
            }
        }
    }
}

/// Validate and durably bind an external instruction to one open reservation.
pub async fn bind_instruction(
    config: &Config,
    rest: &RestClient,
    instruction: &Event,
    channel_id: &str,
) -> anyhow::Result<()> {
    let agent_hex = config.keys.public_key().to_hex();
    let payer = instruction.pubkey;
    let payer_hex = payer.to_hex();
    check_rate_limit(
        &PAID_INVOCATION_LIMITER,
        &format!("{}:{}", payer_hex, channel_id),
        &instruction.id.to_hex(),
        MAX_PAID_INVOCATIONS_PER_WINDOW,
    )?;
    if config.price_per_minute_sats.is_none() || !config.respond_to_allowlist.contains(&payer_hex) {
        return Err(protocol_error("paid runtime is unavailable"));
    }
    let reservation_id = runtime_tag_reservation(instruction, &agent_hex)?;
    let reservation_event_id = EventId::from_hex(&reservation_id)?;
    let rows = query_events(
        rest,
        nostr::Filter::new()
            .id(reservation_event_id)
            .kind(Kind::Custom(KIND_AGENT_RUNTIME_RESERVATION as u16)),
    )
    .await?;
    let reservation_event = rows
        .into_iter()
        .find(|event| event.id == reservation_event_id)
        .ok_or_else(|| protocol_error("runtime reservation is unavailable"))?;
    reservation_event.verify()?;
    if reservation_event.pubkey != config.keys.public_key()
        || exactly_one_tag(&reservation_event, "p")? != payer_hex
        || exactly_one_tag(&reservation_event, "h")? != channel_id
        || exactly_one_tag(&reservation_event, "encryption")? != "nip44_v2"
    {
        return Err(protocol_error("runtime reservation routing does not match"));
    }
    let reservation: RuntimeReservation =
        decrypt_agent_event(&config.keys, &reservation_event, &payer)?;
    reservation.validate()?;
    let tagged_expiration = exactly_one_tag(&reservation_event, "expiration")?
        .parse::<u64>()
        .map_err(|_| protocol_error("runtime reservation expiration is invalid"))?;
    if tagged_expiration != reservation.must_start_by
        || reservation_event.created_at.as_secs() > reservation.must_start_by
        || reservation.must_start_by
            > reservation_event
                .created_at
                .as_secs()
                .saturating_add(REQUEST_TTL_SECS)
    {
        return Err(protocol_error(
            "runtime reservation validity interval does not match its signed terms",
        ));
    }
    let (ledger, _, _) = replay_ledger(&config.keys, rest, &payer, channel_id).await?;
    if reservation.must_start_by < now_secs() {
        publish_settlement(
            &config.keys,
            rest,
            &BoundReservation {
                reservation_id,
                request_id: reservation.request_id,
                instruction_event_id: String::new(),
                payer_pubkey: payer_hex,
                channel_id: channel_id.to_string(),
                cap_ms: reservation.cap_ms,
            },
            0,
            RuntimeOutcome::UnusedExpired,
            false,
        )
        .await?;
        return Err(protocol_error("runtime reservation expired"));
    }
    if !ledger.reservation_is_open(&reservation_event.id.to_hex())
        || ledger.reservation_cap_ms(&reservation_event.id.to_hex()) != Some(reservation.cap_ms)
    {
        return Err(protocol_error("runtime reservation is not open"));
    }

    let binding = BoundReservation {
        reservation_id: reservation_event.id.to_hex(),
        request_id: reservation.request_id,
        instruction_event_id: instruction.id.to_hex(),
        payer_pubkey: payer_hex,
        channel_id: channel_id.to_string(),
        cap_ms: reservation.cap_ms,
    };
    let _created = persist_binding_once(&config.keys, &binding)?;
    crate::runtime_conformance::record_binding(
        trace_scope(&config.keys, &binding),
        &binding.reservation_id,
        &binding.instruction_event_id,
    );
    Ok(())
}

fn trace_scope(
    keys: &nostr::Keys,
    binding: &BoundReservation,
) -> buzz_conformance::paid_agent_runtime::RuntimeOpaqueId {
    crate::runtime_conformance::scope_id(
        &keys.public_key().to_hex(),
        &binding.payer_pubkey,
        &binding.channel_id,
    )
}

/// A reservation meter whose clock starts exactly at the ACP prompt boundary.
pub struct PaidRuntimeMeter {
    binding: BoundReservation,
    keys: nostr::Keys,
    rest: RestClient,
    started: Instant,
    prior_used_ms: u64,
    last_checkpoint_ms: Arc<AtomicU64>,
    finished: Arc<AtomicBool>,
    checkpoint_task: Option<JoinHandle<()>>,
}

impl PaidRuntimeMeter {
    /// Load the reservation bound to this batch and close any additional batched
    /// reservations unused. This does not start billing.
    pub async fn prepare(
        keys: &nostr::Keys,
        rest: &RestClient,
        events: &[Event],
    ) -> anyhow::Result<Option<BoundReservation>> {
        let result = Self::prepare_checked(keys, rest, events).await;
        if result.is_err() {
            let agent_hex = keys.public_key().to_hex();
            for event in events.iter().filter(|event| {
                event.tags.iter().any(|tag| {
                    let parts = tag.as_slice();
                    parts
                        .first()
                        .is_some_and(|value| value.as_str() == "agent_runtime")
                        && parts.get(1).is_none_or(|value| value.as_str() == agent_hex)
                })
            }) {
                let channel = event
                    .tags
                    .iter()
                    .find_map(|tag| {
                        let parts = tag.as_slice();
                        (parts.len() == 2 && parts[0].as_str() == "h").then(|| parts[1].as_str())
                    })
                    .unwrap_or("invalid-channel");
                crate::runtime_conformance::record(
                    crate::runtime_conformance::scope_id(
                        &agent_hex,
                        &event.pubkey.to_hex(),
                        channel,
                    ),
                    buzz_conformance::paid_agent_runtime::RuntimeTraceAction::InvocationRejected,
                );
            }
        }
        result
    }

    async fn prepare_checked(
        keys: &nostr::Keys,
        rest: &RestClient,
        events: &[Event],
    ) -> anyhow::Result<Option<BoundReservation>> {
        let agent_hex = keys.public_key().to_hex();
        let mut bindings = BTreeMap::<String, BoundReservation>::new();
        for event in events {
            let Some(reservation_id) = runtime_tag_reservation_optional(event, &agent_hex)? else {
                continue;
            };
            let binding = load_json::<BoundReservation>(&binding_path(keys, &reservation_id))?
                .ok_or_else(|| {
                    protocol_error("paid instruction has no durable reservation binding")
                })?;
            let event_channel = exactly_one_tag(event, "h")?;
            if binding.reservation_id != reservation_id
                || binding.instruction_event_id != event.id.to_hex()
                || binding.payer_pubkey != event.pubkey.to_hex()
                || binding.channel_id != event_channel
            {
                return Err(protocol_error(
                    "paid instruction does not match its durable reservation binding",
                ));
            }
            if settlement_path(keys, &reservation_id).exists() {
                return Err(protocol_error(
                    "paid instruction reservation is already settled",
                ));
            }
            let payer = PublicKey::from_hex(&binding.payer_pubkey)?;
            let (ledger, _, _) = replay_ledger(keys, rest, &payer, &binding.channel_id).await?;
            if !ledger.reservation_is_open(&reservation_id)
                || ledger.reservation_cap_ms(&reservation_id) != Some(binding.cap_ms)
            {
                return Err(protocol_error("paid instruction reservation is not open"));
            }
            if let Some(existing) = bindings.insert(reservation_id.clone(), binding.clone()) {
                if existing != binding {
                    return Err(protocol_error(
                        "paid batch reuses one reservation for conflicting instructions",
                    ));
                }
            }
        }
        let mut bindings = bindings.into_values().collect::<Vec<_>>();
        bindings.sort_by(|left, right| {
            (&left.instruction_event_id, &left.reservation_id)
                .cmp(&(&right.instruction_event_id, &right.reservation_id))
        });
        let Some(primary) = bindings.first().cloned() else {
            return Ok(None);
        };
        for unused in bindings.into_iter().skip(1) {
            publish_settlement(keys, rest, &unused, 0, RuntimeOutcome::Cancelled, true).await?;
        }
        crate::runtime_conformance::record_dispatch(
            trace_scope(keys, &primary),
            &primary.reservation_id,
            &primary.instruction_event_id,
        );
        Ok(Some(primary))
    }

    /// Persist the running state, start the monotonic clock, and arm one-second checkpoints.
    pub fn start(
        keys: nostr::Keys,
        rest: RestClient,
        binding: BoundReservation,
    ) -> anyhow::Result<Self> {
        let scope = trace_scope(&keys, &binding);
        let result = Self::start_checked(keys, rest, binding);
        if result.is_err() {
            crate::runtime_conformance::record(
                scope,
                buzz_conformance::paid_agent_runtime::RuntimeTraceAction::InvocationRejected,
            );
        }
        result
    }

    fn start_checked(
        keys: nostr::Keys,
        rest: RestClient,
        binding: BoundReservation,
    ) -> anyhow::Result<Self> {
        let prior_checkpoint =
            load_json::<RuntimeCheckpoint>(&checkpoint_path(&keys, &binding.reservation_id))?;
        if prior_checkpoint
            .as_ref()
            .is_some_and(|checkpoint| checkpoint.binding != binding)
        {
            return Err(protocol_error(
                "runtime checkpoint does not match the admitted instruction",
            ));
        }
        let prior_used_ms = prior_checkpoint
            .map_or(0, |checkpoint| checkpoint.checkpointed_used_ms)
            .min(binding.cap_ms);
        acquire_execution_lease(&keys, &binding)?;
        let checkpoint = RuntimeCheckpoint {
            binding: binding.clone(),
            checkpointed_used_ms: prior_used_ms,
        };
        if let Err(error) = atomic_write(
            &checkpoint_path(&keys, &binding.reservation_id),
            &serde_json::to_vec(&checkpoint)?,
        ) {
            release_execution_lease(&keys, &binding.reservation_id, false);
            return Err(error);
        }
        crate::runtime_conformance::record(
            trace_scope(&keys, &binding),
            if prior_used_ms == 0 {
                buzz_conformance::paid_agent_runtime::RuntimeTraceAction::MeterStarted {
                    reservation_id: crate::runtime_conformance::entity_id(
                        "reservation",
                        &binding.reservation_id,
                    ),
                }
            } else {
                buzz_conformance::paid_agent_runtime::RuntimeTraceAction::MeterResumed {
                    reservation_id: crate::runtime_conformance::entity_id(
                        "reservation",
                        &binding.reservation_id,
                    ),
                }
            },
        );
        let started = Instant::now();
        let last_checkpoint_ms = Arc::new(AtomicU64::new(prior_used_ms));
        let checkpoint_counter = Arc::clone(&last_checkpoint_ms);
        let checkpoint_keys = keys.clone();
        let checkpoint_binding = binding.clone();
        let finished = Arc::new(AtomicBool::new(false));
        let task_finished = Arc::clone(&finished);
        let checkpoint_task = tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(1));
            interval.tick().await;
            loop {
                interval.tick().await;
                if task_finished.load(Ordering::Acquire) {
                    break;
                }
                let used_ms = prior_used_ms
                    .saturating_add(
                        u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
                    )
                    .min(checkpoint_binding.cap_ms);
                let checkpoint = RuntimeCheckpoint {
                    binding: checkpoint_binding.clone(),
                    checkpointed_used_ms: used_ms,
                };
                if atomic_write(
                    &checkpoint_path(&checkpoint_keys, &checkpoint_binding.reservation_id),
                    &serde_json::to_vec(&checkpoint).unwrap_or_default(),
                )
                .is_ok()
                {
                    checkpoint_counter.store(used_ms, Ordering::Release);
                    crate::runtime_conformance::record(
                        trace_scope(&checkpoint_keys, &checkpoint_binding),
                        buzz_conformance::paid_agent_runtime::RuntimeTraceAction::MeterCheckpointed {
                            reservation_id: crate::runtime_conformance::entity_id(
                                "reservation",
                                &checkpoint_binding.reservation_id,
                            ),
                            elapsed_ms: used_ms,
                        },
                    );
                }
            }
        });
        Ok(Self {
            binding,
            keys,
            rest,
            started,
            prior_used_ms,
            last_checkpoint_ms,
            finished,
            checkpoint_task: Some(checkpoint_task),
        })
    }

    /// Remaining reservation duration for a budget-deadline select arm.
    pub fn remaining(&self) -> Duration {
        Duration::from_millis(self.binding.cap_ms.saturating_sub(self.prior_used_ms))
            .saturating_sub(self.started.elapsed())
    }

    /// Close the reservation with exact monotonic elapsed time.
    pub async fn finish(mut self, outcome: RuntimeOutcome) -> anyhow::Result<u64> {
        self.finished.store(true, Ordering::Release);
        if let Some(task) = self.checkpoint_task.take() {
            task.abort();
        }
        let used_ms = if outcome == RuntimeOutcome::BudgetExhausted {
            self.binding.cap_ms
        } else {
            self.prior_used_ms
                .saturating_add(
                    u64::try_from(self.started.elapsed().as_millis()).unwrap_or(u64::MAX),
                )
                .min(self.binding.cap_ms)
        };
        let result = publish_settlement(
            &self.keys,
            &self.rest,
            &self.binding,
            used_ms,
            outcome,
            true,
        )
        .await;
        release_execution_lease(&self.keys, &self.binding.reservation_id, result.is_ok());
        result?;
        Ok(used_ms)
    }

    /// Stop billing between automatic retry segments without closing the reservation.
    pub fn pause(mut self) -> anyhow::Result<u64> {
        self.finished.store(true, Ordering::Release);
        if let Some(task) = self.checkpoint_task.take() {
            task.abort();
        }
        let used_ms = self
            .prior_used_ms
            .saturating_add(u64::try_from(self.started.elapsed().as_millis()).unwrap_or(u64::MAX))
            .min(self.binding.cap_ms);
        let checkpoint = RuntimeCheckpoint {
            binding: self.binding.clone(),
            checkpointed_used_ms: used_ms,
        };
        atomic_write(
            &checkpoint_path(&self.keys, &self.binding.reservation_id),
            &serde_json::to_vec(&checkpoint)?,
        )?;
        self.last_checkpoint_ms.store(used_ms, Ordering::Release);
        crate::runtime_conformance::record(
            trace_scope(&self.keys, &self.binding),
            buzz_conformance::paid_agent_runtime::RuntimeTraceAction::MeterCheckpointed {
                reservation_id: crate::runtime_conformance::entity_id(
                    "reservation",
                    &self.binding.reservation_id,
                ),
                elapsed_ms: used_ms,
            },
        );
        crate::runtime_conformance::record(
            trace_scope(&self.keys, &self.binding),
            buzz_conformance::paid_agent_runtime::RuntimeTraceAction::MeterPaused {
                reservation_id: crate::runtime_conformance::entity_id(
                    "reservation",
                    &self.binding.reservation_id,
                ),
            },
        );
        release_execution_lease(&self.keys, &self.binding.reservation_id, false);
        Ok(used_ms)
    }
}

impl Drop for PaidRuntimeMeter {
    fn drop(&mut self) {
        if self.finished.swap(true, Ordering::AcqRel) {
            return;
        }
        if let Some(task) = self.checkpoint_task.take() {
            task.abort();
        }
        let used_ms = self.last_checkpoint_ms.load(Ordering::Acquire);
        let keys = self.keys.clone();
        let rest = self.rest.clone();
        let binding = self.binding.clone();
        tokio::spawn(async move {
            let result = publish_settlement(
                &keys,
                &rest,
                &binding,
                used_ms,
                RuntimeOutcome::Interrupted,
                true,
            )
            .await;
            release_execution_lease(&keys, &binding.reservation_id, result.is_ok());
            if let Err(error) = result {
                tracing::error!(error = %error, "publish interrupted runtime settlement");
            }
        });
    }
}

async fn publish_settlement(
    keys: &nostr::Keys,
    rest: &RestClient,
    binding: &BoundReservation,
    used_ms: u64,
    outcome: RuntimeOutcome,
    include_instruction: bool,
) -> anyhow::Result<Event> {
    let path = settlement_path(keys, &binding.reservation_id);
    let (event, created) = if let Some(existing) = load_json::<Event>(&path)? {
        validate_settlement_event(keys, &existing, Some(binding))?;
        (existing, false)
    } else {
        let settlement = RuntimeSettlement {
            version: VERSION,
            reservation_id: binding.reservation_id.clone(),
            instruction_event_id: include_instruction
                .then(|| binding.instruction_event_id.clone())
                .filter(|value| !value.is_empty()),
            cap_ms: binding.cap_ms,
            used_ms,
            outcome,
        };
        settlement.validate()?;
        let payer = PublicKey::from_hex(&binding.payer_pubkey)?;
        let ciphertext = nip44::encrypt(
            keys.secret_key(),
            &payer,
            serde_json::to_string(&settlement)?,
            Version::V2,
        )?;
        let event = EventBuilder::new(
            Kind::Custom(KIND_AGENT_RUNTIME_SETTLEMENT as u16),
            ciphertext,
        )
        .tags([
            Tag::parse(["p", binding.payer_pubkey.as_str()])?,
            Tag::parse(["h", binding.channel_id.as_str()])?,
            Tag::parse(["e", binding.reservation_id.as_str()])?,
            Tag::parse(["encryption", "nip44_v2"])?,
        ])
        .sign_with_keys(keys)?;
        if create_once(&path, event.as_json().as_bytes())? {
            (event, true)
        } else {
            let existing = load_json::<Event>(&path)?
                .ok_or_else(|| protocol_error("runtime settlement disappeared"))?;
            validate_settlement_event(keys, &existing, Some(binding))?;
            (existing, false)
        }
    };
    crate::runtime_conformance::record(
        trace_scope(keys, binding),
        if created {
            buzz_conformance::paid_agent_runtime::RuntimeTraceAction::ReservationSettled {
                reservation_id: crate::runtime_conformance::entity_id(
                    "reservation",
                    &binding.reservation_id,
                ),
                used_ms,
                outcome: trace_outcome(outcome),
            }
        } else {
            buzz_conformance::paid_agent_runtime::RuntimeTraceAction::DuplicateReused {
                entity_id: crate::runtime_conformance::entity_id(
                    "reservation",
                    &binding.reservation_id,
                ),
            }
        },
    );
    let ack_path = settlement_ack_path(keys, &binding.reservation_id);
    let accepted_id = load_json::<String>(&ack_path)?;
    if accepted_id.as_deref() != Some(event.id.to_hex().as_str()) {
        rest.submit_event(&event).await?;
        atomic_write(&ack_path, &serde_json::to_vec(&event.id.to_hex())?)?;
        tracing::info!(
            reservation_id = %binding.reservation_id,
            used_ms,
            cap_ms = binding.cap_ms,
            outcome = ?outcome,
            "paid runtime settlement accepted"
        );
    }
    Ok(event)
}

fn validate_settlement_event(
    keys: &nostr::Keys,
    event: &Event,
    expected: Option<&BoundReservation>,
) -> anyhow::Result<RuntimeSettlement> {
    event.verify()?;
    if event.pubkey != keys.public_key()
        || event.kind.as_u16() as u32 != KIND_AGENT_RUNTIME_SETTLEMENT
    {
        return Err(protocol_error(
            "invalid durable runtime settlement author or kind",
        ));
    }
    let payer_hex = exactly_one_tag(event, "p")?;
    let channel_id = exactly_one_tag(event, "h")?;
    let reservation_id = exactly_one_tag(event, "e")?;
    let payer = PublicKey::from_hex(payer_hex)?;
    let settlement: RuntimeSettlement = decrypt_agent_event(keys, event, &payer)?;
    settlement.validate()?;
    if settlement.reservation_id != reservation_id {
        return Err(protocol_error(
            "durable settlement does not reference its tagged reservation",
        ));
    }
    if let Some(binding) = expected {
        if payer_hex != binding.payer_pubkey
            || channel_id != binding.channel_id
            || reservation_id != binding.reservation_id
            || settlement.cap_ms != binding.cap_ms
            || settlement.instruction_event_id.as_deref()
                != (!binding.instruction_event_id.is_empty())
                    .then_some(binding.instruction_event_id.as_str())
        {
            return Err(protocol_error(
                "durable settlement does not match its reservation binding",
            ));
        }
    }
    Ok(settlement)
}

/// Retry every locally durable settlement with its exact original signature.
pub async fn retry_persisted_settlements(
    keys: &nostr::Keys,
    rest: &RestClient,
) -> anyhow::Result<()> {
    let entries = match std::fs::read_dir(runtime_directory(keys)) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error.into()),
    };
    for entry in entries {
        let path = entry?.path();
        if !path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.starts_with("settlement-"))
        {
            continue;
        }
        let event = load_json::<Event>(&path)?
            .ok_or_else(|| protocol_error("durable runtime settlement disappeared"))?;
        validate_settlement_event(keys, &event, None)?;
        let reservation_id = exactly_one_tag(&event, "e")?;
        let ack_path = settlement_ack_path(keys, reservation_id);
        if load_json::<String>(&ack_path)?.as_deref() == Some(event.id.to_hex().as_str()) {
            continue;
        }
        rest.submit_event(&event).await?;
        atomic_write(&ack_path, &serde_json::to_vec(&event.id.to_hex())?)?;
    }
    Ok(())
}

/// Close prompt segments left running by an earlier harness process.
///
/// Only the last durable one-second checkpoint is charged. An exact signed
/// settlement is persisted before publication, so startup retries reuse it.
pub async fn recover_interrupted(config: &Config, rest: &RestClient) -> anyhow::Result<()> {
    let keys = &config.keys;
    retry_persisted_settlements(keys, rest).await?;
    let directory = runtime_directory(keys);
    let entries = match std::fs::read_dir(directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error.into()),
    };
    for entry in entries {
        let path = entry?.path();
        if !path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.starts_with("checkpoint-"))
        {
            continue;
        }
        let Some(checkpoint) = load_json::<RuntimeCheckpoint>(&path)? else {
            continue;
        };
        if settlement_path(keys, &checkpoint.binding.reservation_id).exists() {
            release_execution_lease(keys, &checkpoint.binding.reservation_id, true);
            continue;
        }
        let payer = PublicKey::from_hex(&checkpoint.binding.payer_pubkey)?;
        let _ = replay_ledger(&config.keys, rest, &payer, &checkpoint.binding.channel_id).await?;
        publish_settlement(
            keys,
            rest,
            &checkpoint.binding,
            checkpoint
                .checkpointed_used_ms
                .min(checkpoint.binding.cap_ms),
            RuntimeOutcome::Interrupted,
            true,
        )
        .await?;
        release_execution_lease(keys, &checkpoint.binding.reservation_id, true);
    }
    Ok(())
}

/// Return the full reservation for an instruction delivered as a native steer.
pub async fn settle_instruction_unused(
    keys: &nostr::Keys,
    rest: &RestClient,
    instruction_event_id: &str,
) -> anyhow::Result<()> {
    let entries = match std::fs::read_dir(runtime_directory(keys)) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error.into()),
    };
    for entry in entries {
        let path = entry?.path();
        if !path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.starts_with("binding-"))
        {
            continue;
        }
        let Some(binding) = load_json::<BoundReservation>(&path)? else {
            continue;
        };
        if binding.instruction_event_id == instruction_event_id
            && !settlement_path(keys, &binding.reservation_id).exists()
        {
            publish_settlement(keys, rest, &binding, 0, RuntimeOutcome::Cancelled, true).await?;
        }
    }
    Ok(())
}

/// Close a reservation after the queue gives up retrying its instruction.
pub async fn settle_instruction_checkpoint(
    keys: &nostr::Keys,
    rest: &RestClient,
    instruction_event_id: &str,
    outcome: RuntimeOutcome,
) -> anyhow::Result<()> {
    let entries = match std::fs::read_dir(runtime_directory(keys)) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error.into()),
    };
    for entry in entries {
        let path = entry?.path();
        if !path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.starts_with("binding-"))
        {
            continue;
        }
        let Some(binding) = load_json::<BoundReservation>(&path)? else {
            continue;
        };
        if binding.instruction_event_id != instruction_event_id
            || settlement_path(keys, &binding.reservation_id).exists()
        {
            continue;
        }
        let used_ms =
            load_json::<RuntimeCheckpoint>(&checkpoint_path(keys, &binding.reservation_id))?
                .map_or(0, |checkpoint| checkpoint.checkpointed_used_ms)
                .min(binding.cap_ms);
        publish_settlement(keys, rest, &binding, used_ms, outcome, true).await?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rate_limit_is_idempotent_and_bounded() {
        let started = Instant::now();
        let mut limiter = SlidingWindowLimiter::default();
        assert!(limiter.check("payer:channel", "request-a", 2, started));
        assert!(limiter.check("payer:channel", "request-a", 2, started));
        assert!(limiter.check("payer:channel", "request-b", 2, started));
        assert!(!limiter.check("payer:channel", "request-c", 2, started));
        assert!(limiter.check("payer:channel", "request-c", 2, started + RATE_LIMIT_WINDOW));
    }

    #[test]
    fn create_once_never_overwrites_the_winner() {
        let directory = std::env::temp_dir().join(format!(
            "buzz-paid-runtime-create-once-{}",
            uuid::Uuid::new_v4()
        ));
        let path = directory.join("binding.json");
        assert!(create_once(&path, b"first").expect("first create"));
        assert!(!create_once(&path, b"second").expect("duplicate create"));
        assert_eq!(std::fs::read(&path).expect("read winner"), b"first");
        std::fs::remove_dir_all(directory).expect("remove test directory");
    }
}

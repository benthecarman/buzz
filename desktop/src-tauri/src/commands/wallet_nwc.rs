#[cfg(feature = "bitcoin")]
mod enabled {
    mod policy;

    use buzz_core_pkg::{
        kind::{KIND_BOLT12_ZAP_INTENT, KIND_NWC_INFO},
        nwc::{
            build_get_balance_response, build_pay_response, decrypt_request, NwcErrorBody,
            NwcGetBalanceResponse, NwcGetBalanceResult, NwcPayRequest, NwcPayResponse,
            NwcPayResult, NwcRequest,
        },
    };
    use nostr::{Event, EventBuilder, JsonUtil, Kind, Tag};
    use tauri::{AppHandle, Manager, State};

    use crate::{
        app_state::AppState,
        commands::wallet::enabled::{
            analyze_destination_for_nwc, is_unsupported_wallet_event_kind, send_wallet_payment,
            wallet_status,
        },
        relay::{relay_ws_url_with_override, submit_signed_event_at_with_keys},
        wallet::{
            models::{
                WalletError, WalletNwcClient, WalletNwcDefaultPolicy, WalletNwcDefaultPolicyUpdate,
                WalletNwcHandlingResult, WalletNwcPolicyUpdate, WalletNwcRequest,
                WalletPaymentResult, WalletPaymentStatus, WalletSendRequest,
            },
            send::SendAttemptStore,
            zap::{recipient_offer, validate_offer_event},
        },
    };

    fn now_ms() -> u64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|duration| duration.as_millis().min(u128::from(u64::MAX)) as u64)
            .unwrap_or_default()
    }

    fn visible_balance_msats(remaining_budget: u64, spendable_balance: u64) -> u64 {
        remaining_budget
            .min(spendable_balance)
            .saturating_mul(1_000)
    }

    pub(crate) use policy::authorize_hosted_agent;

    pub(crate) fn reconcile_nwc_budget(
        app: &AppHandle,
        state: &AppState,
        request_id: &str,
        settled: bool,
    ) -> Result<(), WalletError> {
        let keys = state.signing_keys().map_err(WalletError::unavailable)?;
        policy::reconcile_charge(
            app,
            &keys.public_key().to_hex(),
            &relay_ws_url_with_override(state),
            request_id,
            settled,
        )
        .map_err(WalletError::unavailable)
    }

    fn event_tag<'a>(event: &'a Event, name: &str) -> Option<&'a str> {
        event.tags.iter().find_map(|tag| {
            let parts = tag.as_slice();
            (parts.first().map(String::as_str) == Some(name))
                .then(|| parts.get(1).map(String::as_str))
                .flatten()
        })
    }

    fn event_tag_count(event: &Event, name: &str) -> usize {
        event
            .tags
            .iter()
            .filter(|tag| tag.as_slice().first().map(String::as_str) == Some(name))
            .count()
    }

    fn validate_payer_note(
        request: &NwcPayRequest,
        instruction_type: &str,
    ) -> Result<(), WalletError> {
        if instruction_type == "bolt11"
            && request
                .params
                .payer_note
                .as_deref()
                .is_some_and(|note| !note.is_empty())
        {
            return Err(WalletError::new(
                "invalid_payment",
                "BOLT11 invoices do not support payer notes",
            ));
        }
        Ok(())
    }

    fn validate_client_request(
        app: &AppHandle,
        state: &AppState,
        keys: &nostr::Keys,
        raw_event: &serde_json::Value,
    ) -> Result<(Event, NwcRequest, String, String, u64), WalletError> {
        let event = Event::from_json(raw_event.to_string())
            .map_err(|error| WalletError::new("invalid_nwc_request", error.to_string()))?;
        let request = decrypt_request(&event, keys)
            .map_err(|error| WalletError::new("invalid_nwc_request", error.to_string()))?;
        let expiration = event_tag(&event, "expiration")
            .and_then(|value| value.parse::<u64>().ok())
            .ok_or_else(|| {
                WalletError::new("invalid_nwc_request", "request has no valid expiration")
            })?;
        let agent_pubkey = event.pubkey.to_hex();
        let owner_pubkey = keys.public_key().to_hex();
        let managed_name = crate::managed_agents::load_managed_agents(app)
            .map_err(WalletError::unavailable)?
            .into_iter()
            .find(|agent| agent.pubkey == agent_pubkey)
            .map(|agent| agent.name);
        let agent_name = match managed_name {
            Some(name) => Some(name),
            None => policy::authorized_hosted_agent(
                app,
                &owner_pubkey,
                &relay_ws_url_with_override(state),
                &agent_pubkey,
            )
            .map_err(WalletError::unavailable)?,
        }
        .ok_or_else(|| WalletError::new("unauthorized", "unknown NWC client"))?;
        if expiration < now_ms() / 1_000 {
            let request_id = uuid::Uuid::new_v5(&uuid::Uuid::NAMESPACE_OID, event.id.as_bytes());
            let can_resume = matches!(&request, NwcRequest::Pay(_))
                && app
                    .path()
                    .app_data_dir()
                    .map_err(|error| WalletError::unavailable(error.to_string()))
                    .and_then(|data_dir| {
                        SendAttemptStore::new(&data_dir, &owner_pubkey)
                            .load(&request_id.to_string())
                    })?
                    .is_some();
            if !can_resume {
                return Err(WalletError::new(
                    "request_expired",
                    "The wallet request expired",
                ));
            }
        }
        Ok((event, request, agent_pubkey, agent_name, expiration))
    }

    async fn validate_request(
        app: &AppHandle,
        state: &AppState,
        keys: &nostr::Keys,
        raw_event: &serde_json::Value,
    ) -> Result<(Event, WalletNwcRequest), WalletError> {
        let (event, request, agent_pubkey, agent_name, expiration) =
            validate_client_request(app, state, keys, raw_event)?;
        let expires_at_ms = expiration.saturating_mul(1_000);
        let NwcRequest::Pay(params) = request else {
            return Err(WalletError::new(
                "invalid_nwc_request",
                "request is not a payment",
            ));
        };
        if params.amount == Some(0) || params.payment.is_empty() {
            return Err(WalletError::new(
                "invalid_nwc_request",
                "payment must be set and amount must be positive when present",
            ));
        }
        let request = NwcPayRequest {
            method: "pay".into(),
            params,
        };
        let intent_json = request
            .params
            .metadata
            .get("zap_intent")
            .and_then(serde_json::Value::as_str);
        let Some(intent_json) = intent_json else {
            let analysis =
                analyze_destination_for_nwc(app, state, request.params.payment.clone()).await?;
            validate_payer_note(&request, &analysis.instruction_type)?;
            let amount = match request.params.amount {
                Some(amount_msats) => {
                    if amount_msats % 1_000 != 0 {
                        return Err(WalletError::new(
                            "invalid_amount",
                            "Buzz wallets accept whole-satoshi NWC payments",
                        ));
                    }
                    amount_msats / 1_000
                }
                None => analysis.amount.ok_or_else(|| {
                    WalletError::new(
                        "invalid_amount",
                        "The selected payment instruction requires an amount",
                    )
                })?,
            };
            if analysis.amount.is_some_and(|expected| expected != amount) {
                return Err(WalletError::new(
                    "invalid_amount",
                    "payment amount conflicts with the selected instruction",
                ));
            }
            if analysis.min_amount.is_some_and(|minimum| amount < minimum)
                || analysis.max_amount.is_some_and(|maximum| amount > maximum)
            {
                return Err(WalletError::new(
                    "invalid_amount",
                    "payment amount is outside the selected instruction range",
                ));
            }
            if analysis
                .expires_at_ms
                .is_some_and(|expiration| expiration <= now_ms())
            {
                return Err(WalletError::new(
                    "invalid_payment",
                    "payment instruction has expired",
                ));
            }
            let request_id = uuid::Uuid::new_v5(&uuid::Uuid::NAMESPACE_OID, event.id.as_bytes());
            return Ok((
                event.clone(),
                WalletNwcRequest {
                    event_id: event.id.to_hex(),
                    expires_at_ms,
                    agent_pubkey,
                    agent_name,
                    request_type: "payment".into(),
                    instruction_type: analysis.instruction_type,
                    recipient_pubkey: None,
                    amount,
                    comment: analysis.description.unwrap_or_default(),
                    destination: analysis.normalized_destination,
                    payer_note: request.params.payer_note,
                    request_id: request_id.to_string(),
                },
            ));
        };
        let intent = Event::from_json(intent_json)
            .map_err(|error| WalletError::new("invalid_zap", error.to_string()))?;
        intent
            .verify()
            .map_err(|error| WalletError::new("invalid_zap", error.to_string()))?;
        if intent.kind != Kind::Custom(KIND_BOLT12_ZAP_INTENT as u16)
            || intent.pubkey != event.pubkey
        {
            return Err(WalletError::new(
                "invalid_zap",
                "zap intent is not signed by the requesting agent",
            ));
        }
        if event_tag_count(&intent, "p") != 1
            || event_tag_count(&intent, "amount") != 1
            || event_tag_count(&intent, "offer_event") != 1
            || event_tag_count(&intent, "zap_id") != 1
            || event_tag_count(&intent, "e") > 1
            || event_tag_count(&intent, "a") > 1
            || (event_tag_count(&intent, "e") != 0 && event_tag_count(&intent, "a") != 0)
        {
            return Err(WalletError::new(
                "invalid_zap",
                "zap intent does not satisfy NIP-B1 tag cardinality",
            ));
        }
        let zap_id = event_tag(&intent, "zap_id").unwrap_or_default();
        if zap_id.len() < 32
            || !zap_id.len().is_multiple_of(2)
            || !zap_id
                .chars()
                .all(|character| character.is_ascii_hexdigit() && !character.is_ascii_uppercase())
        {
            return Err(WalletError::new(
                "invalid_zap",
                "zap intent has an invalid zap_id",
            ));
        }
        let payer_note = request
            .params
            .payer_note
            .clone()
            .ok_or_else(|| WalletError::new("invalid_zap", "request has no payer note"))?;
        let amount_msats = request
            .params
            .amount
            .ok_or_else(|| WalletError::new("invalid_zap", "zap wallet request has no amount"))?;
        if amount_msats % 1_000 != 0 {
            return Err(WalletError::new(
                "invalid_amount",
                "Buzz wallets accept whole-satoshi NWC payments",
            ));
        }
        if payer_note != format!("nostr:nipB1:{}", intent.id.to_hex())
            || event_tag(&intent, "amount") != Some(amount_msats.to_string().as_str())
        {
            return Err(WalletError::new(
                "invalid_zap",
                "wallet request does not match its zap intent",
            ));
        }
        let recipient_pubkey = event_tag(&intent, "p")
            .map(str::to_string)
            .ok_or_else(|| WalletError::new("invalid_zap", "zap intent has no recipient"))?;
        let offer_event_json = event_tag(&intent, "offer_event")
            .ok_or_else(|| WalletError::new("invalid_zap", "zap intent has no offer event"))?;
        let offer_event = Event::from_json(offer_event_json)
            .map_err(|error| WalletError::new("invalid_zap", error.to_string()))?;
        validate_offer_event(&offer_event, &recipient_pubkey)?;
        let recipient_offer = recipient_offer(&offer_event, &recipient_pubkey)?;
        if offer_event.created_at > intent.created_at {
            return Err(WalletError::new(
                "invalid_zap",
                "offer announcement is newer than the zap intent",
            ));
        }
        let analysis =
            analyze_destination_for_nwc(app, state, request.params.payment.clone()).await?;
        if analysis.instruction_type != "bolt12"
            || analysis.normalized_destination != recipient_offer.offer
        {
            return Err(WalletError::new(
                "invalid_zap",
                "wallet selected an offer that the recipient did not authorize",
            ));
        }
        let request_id = uuid::Uuid::new_v5(&uuid::Uuid::NAMESPACE_OID, event.id.as_bytes());
        let parsed = WalletNwcRequest {
            event_id: event.id.to_hex(),
            expires_at_ms,
            agent_pubkey,
            agent_name,
            request_type: "zap".into(),
            instruction_type: "bolt12".into(),
            recipient_pubkey: Some(recipient_pubkey),
            amount: amount_msats / 1_000,
            comment: intent.content,
            destination: analysis.normalized_destination,
            payer_note: Some(payer_note),
            request_id: request_id.to_string(),
        };
        Ok((event, parsed))
    }

    pub(crate) async fn publish_nwc_info(
        state: &AppState,
        keys: &nostr::Keys,
        relay_api_base_urls: &[String],
        enabled: bool,
    ) -> Result<Vec<String>, WalletError> {
        let mut tags = vec![Tag::parse(["encryption", "nip44_v2"])
            .map_err(|error| WalletError::new("relay_publish_failed", error.to_string()))?];
        if enabled {
            tags.push(
                Tag::parse(["extensions", "321"])
                    .map_err(|error| WalletError::new("relay_publish_failed", error.to_string()))?,
            );
        }
        let event = EventBuilder::new(
            Kind::Custom(KIND_NWC_INFO as u16),
            if enabled { "pay get_balance" } else { "" },
        )
        .tags(tags)
        .sign_with_keys(keys)
        .map_err(|error| WalletError::new("relay_publish_failed", error.to_string()))?;
        let mut warnings = Vec::new();
        for (index, relay) in relay_api_base_urls.iter().enumerate() {
            if let Err(error) = submit_signed_event_at_with_keys(&event, state, relay, keys).await {
                if index == 0 && !is_unsupported_wallet_event_kind(&error) {
                    return Err(WalletError::new(
                        "relay_publish_failed",
                        format!("publish NWC info to active community {relay}: {error}"),
                    ));
                }
                warnings.push(format!("{relay}: {error}"));
            }
        }
        Ok(warnings)
    }

    fn payment_response_result(
        request: &WalletNwcRequest,
        payment: WalletPaymentResult,
    ) -> NwcPayResult {
        let state = match payment.status {
            WalletPaymentStatus::Completed => "settled",
            WalletPaymentStatus::Failed => "failed",
            WalletPaymentStatus::Pending => "pending",
        };
        NwcPayResult {
            transaction_id: payment.payment_id,
            state: state.into(),
            instruction_type: request.instruction_type.clone(),
            amount: payment
                .amount
                .unwrap_or(request.amount)
                .saturating_mul(1_000),
            fees_paid: Some(payment.fees.saturating_mul(1_000)),
            preimage: payment.preimage,
            payer_proof: payment.payer_proof,
            txid: payment.txid,
            failure_reason: (state == "failed").then_some(payment.status_message),
            created_at: payment.created_at_ms / 1_000,
            settled_at: if state == "settled" {
                Some(payment.finalized_at_ms.unwrap_or(payment.created_at_ms) / 1_000)
            } else {
                payment.finalized_at_ms.map(|value| value / 1_000)
            },
        }
    }

    fn signed_pay_response(
        keys: &nostr::Keys,
        request_event: &Event,
        request: &WalletNwcRequest,
        payment: WalletPaymentResult,
    ) -> Result<serde_json::Value, WalletError> {
        let response = NwcPayResponse {
            result_type: "pay".into(),
            error: None,
            result: Some(payment_response_result(request, payment)),
        };
        let signed = build_pay_response(keys, request_event, &response)
            .map_err(|error| WalletError::new("invalid_nwc_response", error.to_string()))?
            .sign_with_keys(keys)
            .map_err(|error| WalletError::new("invalid_nwc_response", error.to_string()))?;
        serde_json::to_value(signed)
            .map_err(|error| WalletError::new("invalid_nwc_response", error.to_string()))
    }

    fn signed_pay_error_response(
        keys: &nostr::Keys,
        request_event: &Event,
        code: impl Into<String>,
        message: impl Into<String>,
    ) -> Result<serde_json::Value, WalletError> {
        let response = NwcPayResponse {
            result_type: "pay".into(),
            error: Some(NwcErrorBody {
                code: code.into(),
                message: message.into(),
            }),
            result: None,
        };
        let signed = build_pay_response(keys, request_event, &response)
            .map_err(|error| WalletError::new("invalid_nwc_response", error.to_string()))?
            .sign_with_keys(keys)
            .map_err(|error| WalletError::new("invalid_nwc_response", error.to_string()))?;
        serde_json::to_value(signed)
            .map_err(|error| WalletError::new("invalid_nwc_response", error.to_string()))
    }

    /// List the agents that can request payments from this community.
    #[tauri::command]
    pub async fn wallet_list_nwc_clients(
        app: AppHandle,
        state: State<'_, AppState>,
    ) -> Result<Vec<WalletNwcClient>, WalletError> {
        let keys = state.signing_keys().map_err(WalletError::unavailable)?;
        let owner_pubkey = keys.public_key().to_hex();
        let community = relay_ws_url_with_override(&state);
        let agents = crate::managed_agents::load_managed_agents(&app)
            .map_err(WalletError::unavailable)?
            .into_iter()
            .map(|agent| (agent.pubkey, agent.name))
            .collect();
        policy::list_clients(&app, &owner_pubkey, &community, agents, now_ms())
    }

    /// Set one agent's manual-approval or automatic-budget policy.
    #[tauri::command]
    pub async fn wallet_set_nwc_policy(
        app: AppHandle,
        state: State<'_, AppState>,
        update: WalletNwcPolicyUpdate,
    ) -> Result<WalletNwcClient, WalletError> {
        let keys = state.signing_keys().map_err(WalletError::unavailable)?;
        let owner_pubkey = keys.public_key().to_hex();
        let community = relay_ws_url_with_override(&state);
        let agent_pubkey = nostr::PublicKey::from_hex(&update.agent_pubkey)
            .map_err(|error| WalletError::new("invalid_agent", error.to_string()))?
            .to_hex();
        let managed_name = crate::managed_agents::load_managed_agents(&app)
            .map_err(WalletError::unavailable)?
            .into_iter()
            .find(|agent| agent.pubkey == agent_pubkey)
            .map(|agent| agent.name);
        let agent_name = match managed_name {
            Some(name) => Some(name),
            None => policy::authorized_hosted_agent(&app, &owner_pubkey, &community, &agent_pubkey)
                .map_err(WalletError::unavailable)?,
        }
        .ok_or_else(|| WalletError::new("unauthorized", "unknown NWC client"))?;
        let normalized = WalletNwcPolicyUpdate {
            agent_pubkey: agent_pubkey.clone(),
            ..update
        };
        policy::set_policy(
            &app,
            &owner_pubkey,
            &community,
            &normalized,
            agent_name,
            now_ms(),
        )
    }

    /// Read the owner's default policy for agents created or claimed later.
    #[tauri::command]
    pub async fn wallet_get_default_nwc_policy(
        app: AppHandle,
        state: State<'_, AppState>,
    ) -> Result<WalletNwcDefaultPolicy, WalletError> {
        let keys = state.signing_keys().map_err(WalletError::unavailable)?;
        policy::default_policy(&app, &keys.public_key().to_hex())
    }

    /// Set the owner's default policy for future agents. Never touches
    /// existing agents' policies.
    #[tauri::command]
    pub async fn wallet_set_default_nwc_policy(
        app: AppHandle,
        state: State<'_, AppState>,
        update: WalletNwcDefaultPolicyUpdate,
    ) -> Result<WalletNwcDefaultPolicy, WalletError> {
        let keys = state.signing_keys().map_err(WalletError::unavailable)?;
        policy::set_default_policy(&app, &keys.public_key().to_hex(), &update)
    }

    /// Apply the owner's default NWC policy to a just-created agent.
    ///
    /// Best-effort: failures are logged and swallowed so agent creation never
    /// fails because of the wallet — the agent simply stays on manual
    /// approval.
    pub(crate) fn apply_default_policy_for_new_agent(app: &AppHandle, agent_pubkey: &str) {
        let state = app.state::<AppState>();
        let Ok(keys) = state.signing_keys() else {
            eprintln!("apply default NWC policy for {agent_pubkey}: identity is locked");
            return;
        };
        let owner_pubkey = keys.public_key().to_hex();
        let community = relay_ws_url_with_override(&state);
        if let Err(error) =
            policy::apply_default_policy(app, &owner_pubkey, &community, agent_pubkey, now_ms())
        {
            eprintln!("apply default NWC policy for {agent_pubkey}: {error}");
        }
    }

    /// Validate and handle one NWC request at the wallet trust boundary.
    #[tauri::command]
    pub async fn wallet_handle_nwc_request(
        app: AppHandle,
        state: State<'_, AppState>,
        event: serde_json::Value,
    ) -> Result<WalletNwcHandlingResult, WalletError> {
        let keys = state.signing_keys().map_err(WalletError::unavailable)?;
        let owner_pubkey = keys.public_key().to_hex();
        let community = relay_ws_url_with_override(&state);
        let (request_event, request, agent_pubkey, _, _) =
            validate_client_request(&app, &state, &keys, &event)?;
        if matches!(request, NwcRequest::GetBalance(_)) {
            let remaining =
                policy::remaining_budget(&app, &owner_pubkey, &community, &agent_pubkey, now_ms())
                    .map_err(WalletError::unavailable)?;
            let spendable = wallet_status(&app, &state).await?.spendable_balance;
            let response = NwcGetBalanceResponse {
                result_type: "get_balance".into(),
                error: None,
                result: Some(NwcGetBalanceResult {
                    balance: visible_balance_msats(remaining, spendable),
                }),
            };
            let signed = build_get_balance_response(&keys, &request_event, &response)
                .map_err(|error| WalletError::new("invalid_nwc_response", error.to_string()))?
                .sign_with_keys(&keys)
                .map_err(|error| WalletError::new("invalid_nwc_response", error.to_string()))?;
            return Ok(WalletNwcHandlingResult {
                action: "respond".into(),
                request: None,
                response: Some(serde_json::to_value(signed).map_err(|error| {
                    WalletError::new("invalid_nwc_response", error.to_string())
                })?),
            });
        }

        let (request_event, request) = validate_request(&app, &state, &keys, &event).await?;
        let reserved = policy::reserve_budget(
            &app,
            &owner_pubkey,
            &community,
            &request.agent_pubkey,
            &request.request_id,
            request.amount,
            now_ms(),
        )
        .map_err(WalletError::unavailable)?;
        if !reserved {
            return Ok(WalletNwcHandlingResult {
                action: "approval_required".into(),
                request: Some(request),
                response: None,
            });
        }

        let payment = send_wallet_payment(
            &app,
            &state,
            WalletSendRequest {
                destination: request.destination.clone(),
                amount: Some(request.amount),
                message: request.payer_note.clone(),
                request_id: request.request_id.clone(),
            },
        )
        .await;
        match payment {
            Ok(payment) => {
                if payment.status == WalletPaymentStatus::Failed {
                    policy::release_budget(
                        &app,
                        &owner_pubkey,
                        &community,
                        &request.agent_pubkey,
                        &request.request_id,
                    )
                    .map_err(WalletError::unavailable)?;
                } else if payment.status == WalletPaymentStatus::Completed {
                    policy::settle_budget(
                        &app,
                        &owner_pubkey,
                        &community,
                        &request.agent_pubkey,
                        &request.request_id,
                    )
                    .map_err(WalletError::unavailable)?;
                }
                let action = match payment.status {
                    WalletPaymentStatus::Completed => "payment_completed",
                    WalletPaymentStatus::Failed => "payment_failed",
                    WalletPaymentStatus::Pending => "payment_pending",
                };
                Ok(WalletNwcHandlingResult {
                    action: action.into(),
                    response: Some(signed_pay_response(
                        &keys,
                        &request_event,
                        &request,
                        payment,
                    )?),
                    request: Some(request),
                })
            }
            Err(error) => {
                if error.code != "payment_status_unknown" {
                    policy::release_budget(
                        &app,
                        &owner_pubkey,
                        &community,
                        &request.agent_pubkey,
                        &request.request_id,
                    )
                    .map_err(WalletError::unavailable)?;
                }
                let code = if error.code == "payment_status_unknown" {
                    "PAYMENT_STATUS_UNKNOWN"
                } else {
                    "PAYMENT_FAILED"
                };
                Ok(WalletNwcHandlingResult {
                    action: "payment_failed".into(),
                    request: Some(request),
                    response: Some(signed_pay_error_response(
                        &keys,
                        &request_event,
                        code,
                        error.message,
                    )?),
                })
            }
        }
    }

    /// Validate and decrypt an agent-authored NWC-321 `pay` request.
    #[tauri::command]
    pub async fn wallet_parse_nwc_request(
        app: AppHandle,
        state: State<'_, AppState>,
        event: serde_json::Value,
    ) -> Result<WalletNwcRequest, WalletError> {
        let keys = state.signing_keys().map_err(WalletError::unavailable)?;
        validate_request(&app, &state, &keys, &event)
            .await
            .map(|(_, request)| request)
    }

    /// Build a signed, encrypted NWC response after approval or denial.
    #[tauri::command]
    pub async fn wallet_build_nwc_response(
        app: AppHandle,
        state: State<'_, AppState>,
        event: serde_json::Value,
        payment: Option<WalletPaymentResult>,
        error_code: Option<String>,
        error_message: Option<String>,
    ) -> Result<serde_json::Value, WalletError> {
        let keys = state.signing_keys().map_err(WalletError::unavailable)?;
        let (request_event, request) = validate_request(&app, &state, &keys, &event).await?;
        let response = match (payment, error_code, error_message) {
            (Some(payment), None, None) => NwcPayResponse {
                result_type: "pay".into(),
                error: None,
                result: Some(payment_response_result(&request, payment)),
            },
            (None, Some(code), Some(message)) => NwcPayResponse {
                result_type: "pay".into(),
                error: Some(NwcErrorBody { code, message }),
                result: None,
            },
            _ => {
                return Err(WalletError::new(
                    "invalid_nwc_response",
                    "provide either a payment result or an error",
                ))
            }
        };
        let signed = build_pay_response(&keys, &request_event, &response)
            .map_err(|error| WalletError::new("invalid_nwc_response", error.to_string()))?
            .sign_with_keys(&keys)
            .map_err(|error| WalletError::new("invalid_nwc_response", error.to_string()))?;
        serde_json::to_value(signed)
            .map_err(|error| WalletError::new("invalid_nwc_response", error.to_string()))
    }

    #[cfg(test)]
    mod tests;
}

#[cfg(feature = "bitcoin")]
pub use enabled::*;

#[cfg(not(feature = "bitcoin"))]
mod disabled {
    use serde::Serialize;

    #[derive(Serialize)]
    pub struct WalletDisabledError {
        code: &'static str,
        message: &'static str,
    }

    fn disabled() -> WalletDisabledError {
        WalletDisabledError {
            code: "wallet_unavailable",
            message: "this Buzz binary was built without the `bitcoin` feature",
        }
    }

    #[tauri::command]
    pub async fn wallet_parse_nwc_request(
        event: serde_json::Value,
    ) -> Result<serde_json::Value, WalletDisabledError> {
        let _ = event;
        Err(disabled())
    }

    #[tauri::command]
    pub async fn wallet_build_nwc_response(
        event: serde_json::Value,
        payment: Option<serde_json::Value>,
        error_code: Option<String>,
        error_message: Option<String>,
    ) -> Result<serde_json::Value, WalletDisabledError> {
        let _ = (event, payment, error_code, error_message);
        Err(disabled())
    }

    #[tauri::command]
    pub async fn wallet_handle_nwc_request(
        event: serde_json::Value,
    ) -> Result<serde_json::Value, WalletDisabledError> {
        let _ = event;
        Err(disabled())
    }

    #[tauri::command]
    pub async fn wallet_list_nwc_clients() -> Result<serde_json::Value, WalletDisabledError> {
        Err(disabled())
    }

    #[tauri::command]
    pub async fn wallet_set_nwc_policy(
        update: serde_json::Value,
    ) -> Result<serde_json::Value, WalletDisabledError> {
        let _ = update;
        Err(disabled())
    }

    #[tauri::command]
    pub async fn wallet_get_default_nwc_policy() -> Result<serde_json::Value, WalletDisabledError> {
        Err(disabled())
    }

    #[tauri::command]
    pub async fn wallet_set_default_nwc_policy(
        update: serde_json::Value,
    ) -> Result<serde_json::Value, WalletDisabledError> {
        let _ = update;
        Err(disabled())
    }
}

#[cfg(not(feature = "bitcoin"))]
pub use disabled::*;

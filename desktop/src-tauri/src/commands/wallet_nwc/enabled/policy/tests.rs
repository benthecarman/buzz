use super::*;

fn budget_default_update(amount: u64, period: &str) -> WalletNwcDefaultPolicyUpdate {
    WalletNwcDefaultPolicyUpdate {
        mode: "budget".into(),
        budget_amount: Some(amount),
        budget_period: Some(period.into()),
    }
}

#[test]
fn materializes_default_budget_once_per_agent() {
    let mut clients = AuthorizedNwcClients {
        default_policy: DefaultNwcPolicy::Budget {
            amount: 500,
            period: NwcBudgetPeriod::Week,
        },
        ..Default::default()
    };
    assert!(materialize_default_policy(
        &mut clients,
        "relay",
        "agent",
        1_000
    ));
    match &clients.policies[&client_key("relay", "agent")] {
        NwcClientPolicy::Budget {
            amount,
            period,
            period_started_at_ms,
            spent,
            charges,
        } => {
            assert_eq!(
                (*amount, *period, *period_started_at_ms, *spent),
                (500, NwcBudgetPeriod::Week, 1_000, 0)
            );
            assert!(charges.is_empty());
        }
        policy => panic!("expected budget policy, got {policy:?}"),
    }
    clients.default_policy = DefaultNwcPolicy::Budget {
        amount: 9_999,
        period: NwcBudgetPeriod::Month,
    };
    assert!(!materialize_default_policy(
        &mut clients,
        "relay",
        "agent",
        2_000
    ));
    match &clients.policies[&client_key("relay", "agent")] {
        NwcClientPolicy::Budget { amount, .. } => assert_eq!(*amount, 500),
        policy => panic!("expected budget policy, got {policy:?}"),
    }
}

#[test]
fn manual_default_materializes_nothing() {
    let mut clients = AuthorizedNwcClients {
        default_policy: DefaultNwcPolicy::Manual,
        ..Default::default()
    };
    assert!(!materialize_default_policy(
        &mut clients,
        "relay",
        "agent",
        1_000
    ));
    assert!(clients.policies.is_empty());
}

#[test]
fn old_authorization_files_default_to_manual_policy() {
    let decoded: AuthorizedNwcClients =
        serde_json::from_str(r#"{"clients":{"relay\\nagent":{"name":"Agent"}}}"#).unwrap();
    assert!(decoded.policies.is_empty());
    assert_eq!(decoded.default_policy, DefaultNwcPolicy::Manual);
}

#[test]
fn authorize_at_applies_default_for_new_hosted_agents_only() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("clients.json");
    let clients = AuthorizedNwcClients {
        default_policy: DefaultNwcPolicy::Budget {
            amount: 300,
            period: NwcBudgetPeriod::Day,
        },
        ..Default::default()
    };
    store_authorized_clients(&path, &clients).unwrap();
    authorize_at(&path, "wss://relay", "agent-a", "Warm Butterfly").unwrap();
    let after_first = load_authorized_clients(&path).unwrap();
    assert!(matches!(
        after_first.policies[&client_key("wss://relay", "agent-a")],
        NwcClientPolicy::Budget {
            amount: 300,
            spent: 0,
            ..
        }
    ));
    let mut after_first = after_first;
    if let Some(NwcClientPolicy::Budget { spent, .. }) = after_first
        .policies
        .get_mut(&client_key("wss://relay", "agent-a"))
    {
        *spent = 120;
    }
    store_authorized_clients(&path, &after_first).unwrap();
    authorize_at(&path, "wss://relay", "agent-a", "Warm Butterfly").unwrap();
    match &load_authorized_clients(&path).unwrap().policies[&client_key("wss://relay", "agent-a")] {
        NwcClientPolicy::Budget { spent, .. } => assert_eq!(*spent, 120),
        policy => panic!("expected budget policy, got {policy:?}"),
    }
}

#[test]
fn factory_reauthorization_materializes_missing_default() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("clients.json");
    let mut clients = AuthorizedNwcClients {
        default_policy: DefaultNwcPolicy::Budget {
            amount: 700,
            period: NwcBudgetPeriod::Week,
        },
        ..Default::default()
    };
    clients.clients.insert(
        client_key("wss://relay", "factory-agent"),
        AuthorizedNwcClient {
            name: "Factory Agent".into(),
        },
    );
    store_authorized_clients(&path, &clients).unwrap();
    authorize_at(&path, "wss://relay", "factory-agent", "Factory Agent").unwrap();
    assert!(matches!(
        load_authorized_clients(&path).unwrap().policies
            [&client_key("wss://relay", "factory-agent")],
        NwcClientPolicy::Budget {
            amount: 700,
            period: NwcBudgetPeriod::Week,
            ..
        }
    ));
}

#[test]
fn default_policy_updates_round_trip_and_validate() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("clients.json");
    let clients = AuthorizedNwcClients {
        default_policy: parse_default_policy(&budget_default_update(250, "month"))
            .expect("valid default update"),
        ..Default::default()
    };
    store_authorized_clients(&path, &clients).unwrap();
    assert_eq!(
        load_authorized_clients(&path).unwrap().default_policy,
        DefaultNwcPolicy::Budget {
            amount: 250,
            period: NwcBudgetPeriod::Month
        }
    );
    let invalid = [
        WalletNwcDefaultPolicyUpdate {
            mode: "auto".into(),
            budget_amount: None,
            budget_period: None,
        },
        budget_default_update(0, "day"),
        WalletNwcDefaultPolicyUpdate {
            mode: "budget".into(),
            budget_amount: Some(100),
            budget_period: None,
        },
        budget_default_update(100, "year"),
    ];
    for update in invalid {
        let error = parse_default_policy(&update).unwrap_err();
        assert_eq!(error.code, "invalid_budget");
    }
}

#[test]
fn budget_reservations_are_atomic_and_idempotent() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("clients.json");
    let update = WalletNwcPolicyUpdate {
        agent_pubkey: "agent".into(),
        mode: "budget".into(),
        budget_amount: Some(100),
        budget_period: Some("day".into()),
    };
    let policy = NwcClientPolicy::Budget {
        amount: 100,
        period: NwcBudgetPeriod::Day,
        period_started_at_ms: 1,
        spent: 0,
        charges: BTreeMap::new(),
    };
    let mut clients = AuthorizedNwcClients::default();
    clients
        .policies
        .insert(client_key("relay", &update.agent_pubkey), policy);
    store_authorized_clients(&path, &clients).unwrap();
    assert!(reserve_budget_at(&path, "relay", "agent", "one", 60, 1).unwrap());
    assert!(reserve_budget_at(&path, "relay", "agent", "one", 60, 1).unwrap());
    assert!(!reserve_budget_at(&path, "relay", "agent", "two", 41, 1).unwrap());
    assert!(reserve_budget_at(&path, "relay", "agent", "two", 40, 1).unwrap());
}

#[test]
fn budget_edits_preserve_current_spend_and_charges() {
    let mut charges = BTreeMap::new();
    charges.insert("pending".into(), NwcBudgetCharge::pending(40));
    charges.insert(
        "settled".into(),
        NwcBudgetCharge::Current {
            amount: 30,
            settled: true,
        },
    );
    let mut current = NwcClientPolicy::Budget {
        amount: 100,
        period: NwcBudgetPeriod::Day,
        period_started_at_ms: 1_000,
        spent: 70,
        charges: charges.clone(),
    };

    let edited = edit_budget_policy(&mut current, 200, NwcBudgetPeriod::Week, 2_000);

    assert_eq!(
        edited,
        NwcClientPolicy::Budget {
            amount: 200,
            period: NwcBudgetPeriod::Week,
            period_started_at_ms: 1_000,
            spent: 70,
            charges,
        }
    );
}

#[test]
fn authorizes_hosted_agents_by_identity() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("clients.json");
    authorize_at(&path, "wss://relay", "agent-a", "Warm Butterfly").unwrap();
    authorize_at(&path, "wss://relay", "agent-a", "Warm Butterfly").unwrap();
    authorize_at(&path, "wss://relay", "agent-b", "Radiant Otter").unwrap();
    let clients = load_authorized_clients(&path).unwrap();
    assert_eq!(
        clients
            .clients
            .get(&client_key("wss://relay", "agent-a"))
            .map(|client| client.name.as_str()),
        Some("Warm Butterfly")
    );
}

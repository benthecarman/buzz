use std::collections::BTreeMap;

use super::*;

#[test]
fn elapsed_budget_period_preserves_pending_reservations() {
    let mut policy = NwcClientPolicy::Budget {
        amount: 100,
        period: NwcBudgetPeriod::Hour,
        period_started_at_ms: 1_000,
        spent: 90,
        charges: BTreeMap::from([
            ("pending".into(), NwcBudgetCharge::pending(80)),
            (
                "settled".into(),
                NwcBudgetCharge::Current {
                    amount: 10,
                    settled: true,
                },
            ),
        ]),
    };

    assert!(policy.reset_if_elapsed(3_601_000));
    assert_eq!(policy.remaining(), Some(20));
    let NwcClientPolicy::Budget { spent, charges, .. } = policy else {
        unreachable!();
    };
    assert_eq!(spent, 80);
    assert_eq!(
        charges,
        BTreeMap::from([("pending".into(), NwcBudgetCharge::pending(80))])
    );
}

#[test]
fn settled_reservations_clear_on_the_next_period() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("clients.json");
    let mut clients = AuthorizedNwcClients::default();
    clients.policies.insert(
        client_key("relay", "agent"),
        NwcClientPolicy::Budget {
            amount: 100,
            period: NwcBudgetPeriod::Hour,
            period_started_at_ms: 1_000,
            spent: 80,
            charges: BTreeMap::from([("request".into(), NwcBudgetCharge::pending(80))]),
        },
    );
    store_authorized_clients(&path, &clients).unwrap();

    settle_budget_at(&path, "relay", "agent", "request").unwrap();
    let mut clients = load_authorized_clients(&path).unwrap();
    let policy = clients
        .policies
        .get_mut(&client_key("relay", "agent"))
        .unwrap();
    assert!(policy.reset_if_elapsed(3_601_000));
    assert_eq!(policy.remaining(), Some(100));
}

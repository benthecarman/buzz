use super::*;

fn reconcile_charge_at(
    path: &Path,
    community: &str,
    request_id: &str,
    settled: bool,
) -> Result<(), String> {
    let _guard = authorized_clients_lock()
        .lock()
        .map_err(|error| format!("lock NWC clients: {error}"))?;
    let mut clients = load_authorized_clients(path)?;
    let prefix = format!("{community}\n");
    let mut changed = false;
    for (key, policy) in &mut clients.policies {
        if !key.starts_with(&prefix) {
            continue;
        }
        let NwcClientPolicy::Budget { spent, charges, .. } = policy else {
            continue;
        };
        if settled {
            changed |= charges
                .get_mut(request_id)
                .is_some_and(NwcBudgetCharge::mark_settled);
        } else if let Some(charge) = charges.remove(request_id) {
            *spent = spent.saturating_sub(charge.amount());
            changed = true;
        }
    }
    if changed {
        store_authorized_clients(path, &clients)?;
    }
    Ok(())
}

pub(in super::super) fn reconcile_charge(
    app: &AppHandle,
    owner_pubkey: &str,
    community: &str,
    request_id: &str,
    settled: bool,
) -> Result<(), String> {
    reconcile_charge_at(
        &authorized_clients_path(app, owner_pubkey)?,
        community,
        request_id,
        settled,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settles_completed_and_releases_failed_charges() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("clients.json");
        let mut clients = AuthorizedNwcClients::default();
        clients.policies.insert(
            client_key("relay", "agent"),
            NwcClientPolicy::Budget {
                amount: 100,
                period: NwcBudgetPeriod::Day,
                period_started_at_ms: 1,
                spent: 0,
                charges: BTreeMap::new(),
            },
        );
        store_authorized_clients(&path, &clients).unwrap();
        assert!(reserve_budget_at(&path, "relay", "agent", "paid", 60, 1).unwrap());
        assert!(reserve_budget_at(&path, "relay", "agent", "failed", 40, 1).unwrap());

        reconcile_charge_at(&path, "relay", "paid", true).unwrap();
        reconcile_charge_at(&path, "relay", "failed", false).unwrap();

        let clients = load_authorized_clients(&path).unwrap();
        let NwcClientPolicy::Budget { spent, charges, .. } =
            &clients.policies[&client_key("relay", "agent")]
        else {
            panic!("expected budget policy");
        };
        assert_eq!(*spent, 60);
        assert!(charges
            .get("paid")
            .is_some_and(|charge| !charge.is_pending()));
        assert!(!charges.contains_key("failed"));
    }
}

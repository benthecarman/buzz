use std::{
    collections::BTreeMap,
    io::Write,
    path::{Path, PathBuf},
    sync::{Mutex, OnceLock},
};

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager};

use crate::wallet::models::{
    WalletError, WalletNwcClient, WalletNwcDefaultPolicy, WalletNwcDefaultPolicyUpdate,
    WalletNwcPolicyUpdate,
};

#[derive(Clone, Deserialize, Serialize)]
struct AuthorizedNwcClient {
    name: String,
}

#[derive(Default, Deserialize, Serialize)]
struct AuthorizedNwcClients {
    clients: BTreeMap<String, AuthorizedNwcClient>,
    #[serde(default)]
    policies: BTreeMap<String, NwcClientPolicy>,
    /// Template applied to agents that become NWC clients later. Older files
    /// without this field decode to `Manual`, matching the pre-default behavior.
    #[serde(default)]
    default_policy: DefaultNwcPolicy,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
enum NwcBudgetPeriod {
    Hour,
    Day,
    Week,
    Month,
}

impl NwcBudgetPeriod {
    fn millis(self) -> u64 {
        let days = match self {
            Self::Hour => return 60 * 60 * 1_000,
            Self::Day => 1,
            Self::Week => 7,
            Self::Month => 30,
        };
        days * 24 * 60 * 60 * 1_000
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::Hour => "hour",
            Self::Day => "day",
            Self::Week => "week",
            Self::Month => "month",
        }
    }
}

impl std::str::FromStr for NwcBudgetPeriod {
    type Err = WalletError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "hour" => Ok(Self::Hour),
            "day" => Ok(Self::Day),
            "week" => Ok(Self::Week),
            "month" => Ok(Self::Month),
            _ => Err(WalletError::new(
                "invalid_budget",
                "Budget period must be hour, day, week, or month",
            )),
        }
    }
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "mode", rename_all = "snake_case")]
enum NwcClientPolicy {
    #[default]
    Manual,
    Budget {
        amount: u64,
        period: NwcBudgetPeriod,
        period_started_at_ms: u64,
        spent: u64,
        #[serde(default)]
        charges: BTreeMap<String, NwcBudgetCharge>,
    },
}

/// One idempotent budget charge.
///
/// Legacy numeric entries are treated as unresolved reservations. This is
/// conservative: an old pending payment must continue to consume budget until
/// a retry proves that it settled or failed.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(untagged)]
enum NwcBudgetCharge {
    Current { amount: u64, settled: bool },
    Legacy(u64),
}

impl NwcBudgetCharge {
    const fn pending(amount: u64) -> Self {
        Self::Current {
            amount,
            settled: false,
        }
    }

    const fn amount(&self) -> u64 {
        match self {
            Self::Current { amount, .. } | Self::Legacy(amount) => *amount,
        }
    }

    fn is_pending(&self) -> bool {
        match self {
            Self::Current { settled, .. } => !settled,
            Self::Legacy(_) => true,
        }
    }

    fn mark_settled(&mut self) -> bool {
        if !self.is_pending() {
            return false;
        }
        *self = Self::Current {
            amount: self.amount(),
            settled: true,
        };
        true
    }
}

/// Owner's template policy for agents that become NWC clients later.
///
/// Mirrors [`NwcClientPolicy`]'s serde shape but carries no runtime state:
/// `period_started_at_ms`, `spent`, and `charges` start fresh for each agent
/// the template is materialized into.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "mode", rename_all = "snake_case")]
enum DefaultNwcPolicy {
    #[default]
    Manual,
    Budget {
        amount: u64,
        period: NwcBudgetPeriod,
    },
}

impl NwcClientPolicy {
    fn reset_if_elapsed(&mut self, timestamp_ms: u64) -> bool {
        let Self::Budget {
            period,
            period_started_at_ms,
            spent,
            charges,
            ..
        } = self
        else {
            return false;
        };
        let period_ms = period.millis();
        let elapsed = timestamp_ms.saturating_sub(*period_started_at_ms);
        if elapsed < period_ms {
            return false;
        }
        let periods = elapsed / period_ms;
        *period_started_at_ms =
            period_started_at_ms.saturating_add(periods.saturating_mul(period_ms));
        charges.retain(|_, charge| charge.is_pending());
        *spent = charges
            .values()
            .fold(0u64, |total, charge| total.saturating_add(charge.amount()));
        true
    }

    fn remaining(&self) -> Option<u64> {
        match self {
            Self::Manual => None,
            Self::Budget { amount, spent, .. } => Some(amount.saturating_sub(*spent)),
        }
    }
}

fn edit_budget_policy(
    current: &mut NwcClientPolicy,
    amount: u64,
    period: NwcBudgetPeriod,
    timestamp_ms: u64,
) -> NwcClientPolicy {
    current.reset_if_elapsed(timestamp_ms);
    match current {
        NwcClientPolicy::Budget {
            period_started_at_ms,
            spent,
            charges,
            ..
        } => NwcClientPolicy::Budget {
            amount,
            period,
            period_started_at_ms: *period_started_at_ms,
            spent: *spent,
            charges: charges.clone(),
        },
        NwcClientPolicy::Manual => NwcClientPolicy::Budget {
            amount,
            period,
            period_started_at_ms: timestamp_ms,
            spent: 0,
            charges: BTreeMap::new(),
        },
    }
}

fn authorized_clients_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(Mutex::default)
}

fn authorized_clients_path(app: &AppHandle, owner_pubkey: &str) -> Result<PathBuf, String> {
    app.path()
        .app_data_dir()
        .map(|path| {
            path.join("wallet")
                .join("nwc-clients")
                .join(format!("{owner_pubkey}.json"))
        })
        .map_err(|error| format!("resolve app data path: {error}"))
}

fn load_authorized_clients(path: &Path) -> Result<AuthorizedNwcClients, String> {
    match std::fs::read(path) {
        Ok(bytes) => {
            serde_json::from_slice(&bytes).map_err(|error| format!("decode NWC clients: {error}"))
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            Ok(AuthorizedNwcClients::default())
        }
        Err(error) => Err(format!("read NWC clients: {error}")),
    }
}

fn store_authorized_clients(path: &Path, clients: &AuthorizedNwcClients) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or_else(|| "NWC client path has no parent directory".to_string())?;
    crate::wallet::ensure_private_directory(parent)
        .map_err(|error| format!("create NWC client directory: {error}"))?;
    let bytes =
        serde_json::to_vec(clients).map_err(|error| format!("encode NWC clients: {error}"))?;
    let mut file = crate::wallet::private_atomic_file(path)
        .map_err(|error| format!("open NWC clients: {error}"))?;
    file.write_all(&bytes)
        .map_err(|error| format!("write NWC clients: {error}"))?;
    file.commit()
        .map_err(|error| format!("commit NWC clients: {error}"))
}

fn client_key(community: &str, agent_pubkey: &str) -> String {
    format!("{community}\n{agent_pubkey}")
}

fn policy_snapshot(
    policy: &NwcClientPolicy,
    agent_pubkey: String,
    agent_name: String,
) -> WalletNwcClient {
    match policy {
        NwcClientPolicy::Manual => WalletNwcClient {
            agent_pubkey,
            agent_name,
            mode: "manual".into(),
            budget_amount: None,
            budget_period: None,
            spent_amount: 0,
            remaining_amount: None,
            period_ends_at_ms: None,
        },
        NwcClientPolicy::Budget {
            amount,
            period,
            period_started_at_ms,
            spent,
            ..
        } => WalletNwcClient {
            agent_pubkey,
            agent_name,
            mode: "budget".into(),
            budget_amount: Some(*amount),
            budget_period: Some(period.as_str().into()),
            spent_amount: *spent,
            remaining_amount: Some(amount.saturating_sub(*spent)),
            period_ends_at_ms: Some(period_started_at_ms.saturating_add(period.millis())),
        },
    }
}

fn default_policy_snapshot(default: &DefaultNwcPolicy) -> WalletNwcDefaultPolicy {
    match default {
        DefaultNwcPolicy::Manual => WalletNwcDefaultPolicy {
            mode: "manual".into(),
            budget_amount: None,
            budget_period: None,
        },
        DefaultNwcPolicy::Budget { amount, period } => WalletNwcDefaultPolicy {
            mode: "budget".into(),
            budget_amount: Some(*amount),
            budget_period: Some(period.as_str().into()),
        },
    }
}

fn authorize_at(
    path: &Path,
    community: &str,
    agent_pubkey: &str,
    agent_name: &str,
) -> Result<(), String> {
    let _guard = authorized_clients_lock()
        .lock()
        .map_err(|error| format!("lock NWC clients: {error}"))?;
    let mut clients = load_authorized_clients(path)?;
    let key = client_key(community, agent_pubkey);
    let client_unchanged = clients
        .clients
        .get(&key)
        .is_some_and(|existing| existing.name == agent_name);
    let policy_inserted =
        materialize_default_policy(&mut clients, community, agent_pubkey, super::now_ms());
    if client_unchanged && !policy_inserted {
        return Ok(());
    }
    if !client_unchanged {
        clients.clients.insert(
            key,
            AuthorizedNwcClient {
                name: agent_name.to_string(),
            },
        );
    }
    store_authorized_clients(path, &clients)
}

pub(crate) fn authorize_hosted_agent(
    app: &AppHandle,
    owner_pubkey: &str,
    community: &str,
    agent_pubkey: &str,
    agent_name: &str,
) -> Result<(), String> {
    authorize_at(
        &authorized_clients_path(app, owner_pubkey)?,
        community,
        agent_pubkey,
        agent_name,
    )
}

pub(super) fn authorized_hosted_agent(
    app: &AppHandle,
    owner_pubkey: &str,
    community: &str,
    agent_pubkey: &str,
) -> Result<Option<String>, String> {
    let _guard = authorized_clients_lock()
        .lock()
        .map_err(|error| format!("lock NWC clients: {error}"))?;
    Ok(
        load_authorized_clients(&authorized_clients_path(app, owner_pubkey)?)?
            .clients
            .get(&client_key(community, agent_pubkey))
            .map(|client| client.name.clone()),
    )
}

pub(super) fn list_clients(
    app: &AppHandle,
    owner_pubkey: &str,
    community: &str,
    mut agents: BTreeMap<String, String>,
    timestamp_ms: u64,
) -> Result<Vec<WalletNwcClient>, WalletError> {
    let path = authorized_clients_path(app, owner_pubkey).map_err(WalletError::unavailable)?;
    let _guard = authorized_clients_lock()
        .lock()
        .map_err(|error| WalletError::unavailable(format!("lock NWC clients: {error}")))?;
    let mut clients = load_authorized_clients(&path).map_err(WalletError::unavailable)?;
    let prefix = format!("{community}\n");
    for (key, client) in &clients.clients {
        if let Some(agent_pubkey) = key.strip_prefix(&prefix) {
            agents
                .entry(agent_pubkey.to_string())
                .or_insert_with(|| client.name.clone());
        }
    }
    let mut changed = false;
    let result = agents
        .into_iter()
        .map(|(agent_pubkey, agent_name)| {
            let policy = clients
                .policies
                .entry(client_key(community, &agent_pubkey))
                .or_default();
            changed |= policy.reset_if_elapsed(timestamp_ms);
            policy_snapshot(policy, agent_pubkey, agent_name)
        })
        .collect();
    if changed {
        store_authorized_clients(&path, &clients).map_err(WalletError::unavailable)?;
    }
    Ok(result)
}

pub(super) fn set_policy(
    app: &AppHandle,
    owner_pubkey: &str,
    community: &str,
    update: &WalletNwcPolicyUpdate,
    agent_name: String,
    timestamp_ms: u64,
) -> Result<WalletNwcClient, WalletError> {
    let budget = match update.mode.as_str() {
        "manual" => None,
        "budget" => {
            let amount = update.budget_amount.ok_or_else(|| {
                WalletError::new("invalid_budget", "Enter a budget greater than zero")
            })?;
            if amount == 0 {
                return Err(WalletError::new(
                    "invalid_budget",
                    "Enter a budget greater than zero",
                ));
            }
            let period = update
                .budget_period
                .as_deref()
                .ok_or_else(|| WalletError::new("invalid_budget", "Select a budget period"))?
                .parse()?;
            Some((amount, period))
        }
        _ => {
            return Err(WalletError::new(
                "invalid_budget",
                "Approval mode must be manual or budget",
            ))
        }
    };
    let path = authorized_clients_path(app, owner_pubkey).map_err(WalletError::unavailable)?;
    let _guard = authorized_clients_lock()
        .lock()
        .map_err(|error| WalletError::unavailable(format!("lock NWC clients: {error}")))?;
    let mut clients = load_authorized_clients(&path).map_err(WalletError::unavailable)?;
    let policy = match budget {
        None => NwcClientPolicy::Manual,
        Some((amount, period)) => {
            let current = clients
                .policies
                .entry(client_key(community, &update.agent_pubkey))
                .or_default();
            edit_budget_policy(current, amount, period, timestamp_ms)
        }
    };
    clients
        .policies
        .insert(client_key(community, &update.agent_pubkey), policy.clone());
    store_authorized_clients(&path, &clients).map_err(WalletError::unavailable)?;
    Ok(policy_snapshot(
        &policy,
        update.agent_pubkey.clone(),
        agent_name,
    ))
}

/// Insert the default policy for one agent unless that would be a no-op.
///
/// Returns `true` only when a policy was inserted. A `Manual` default writes
/// nothing (a missing policy already resolves to manual) and an existing
/// per-agent policy is never overwritten, so re-running is safe.
fn materialize_default_policy(
    clients: &mut AuthorizedNwcClients,
    community: &str,
    agent_pubkey: &str,
    timestamp_ms: u64,
) -> bool {
    let DefaultNwcPolicy::Budget { amount, period } = &clients.default_policy else {
        return false;
    };
    let (amount, period) = (*amount, *period);
    let key = client_key(community, agent_pubkey);
    if clients.policies.contains_key(&key) {
        return false;
    }
    clients.policies.insert(
        key,
        NwcClientPolicy::Budget {
            amount,
            period,
            period_started_at_ms: timestamp_ms,
            spent: 0,
            charges: BTreeMap::new(),
        },
    );
    true
}

/// Apply the owner's default policy to one agent, persisting the result.
pub(super) fn apply_default_policy(
    app: &AppHandle,
    owner_pubkey: &str,
    community: &str,
    agent_pubkey: &str,
    timestamp_ms: u64,
) -> Result<bool, WalletError> {
    let path = authorized_clients_path(app, owner_pubkey).map_err(WalletError::unavailable)?;
    let _guard = authorized_clients_lock()
        .lock()
        .map_err(|error| WalletError::unavailable(format!("lock NWC clients: {error}")))?;
    let mut clients = load_authorized_clients(&path).map_err(WalletError::unavailable)?;
    if !materialize_default_policy(&mut clients, community, agent_pubkey, timestamp_ms) {
        return Ok(false);
    }
    store_authorized_clients(&path, &clients).map_err(WalletError::unavailable)?;
    Ok(true)
}

/// Read the owner's default policy for future agents.
pub(super) fn default_policy(
    app: &AppHandle,
    owner_pubkey: &str,
) -> Result<WalletNwcDefaultPolicy, WalletError> {
    let path = authorized_clients_path(app, owner_pubkey).map_err(WalletError::unavailable)?;
    let _guard = authorized_clients_lock()
        .lock()
        .map_err(|error| WalletError::unavailable(format!("lock NWC clients: {error}")))?;
    let clients = load_authorized_clients(&path).map_err(WalletError::unavailable)?;
    Ok(default_policy_snapshot(&clients.default_policy))
}

fn parse_default_policy(
    update: &WalletNwcDefaultPolicyUpdate,
) -> Result<DefaultNwcPolicy, WalletError> {
    match update.mode.as_str() {
        "manual" => Ok(DefaultNwcPolicy::Manual),
        "budget" => {
            let amount = update.budget_amount.ok_or_else(|| {
                WalletError::new("invalid_budget", "Enter a budget greater than zero")
            })?;
            if amount == 0 {
                return Err(WalletError::new(
                    "invalid_budget",
                    "Enter a budget greater than zero",
                ));
            }
            let period = update
                .budget_period
                .as_deref()
                .ok_or_else(|| WalletError::new("invalid_budget", "Select a budget period"))?
                .parse()?;
            Ok(DefaultNwcPolicy::Budget { amount, period })
        }
        _ => Err(WalletError::new(
            "invalid_budget",
            "Approval mode must be manual or budget",
        )),
    }
}

/// Set the owner's default policy for future agents. Existing per-agent
/// policies are never touched.
pub(super) fn set_default_policy(
    app: &AppHandle,
    owner_pubkey: &str,
    update: &WalletNwcDefaultPolicyUpdate,
) -> Result<WalletNwcDefaultPolicy, WalletError> {
    let default = parse_default_policy(update)?;
    let path = authorized_clients_path(app, owner_pubkey).map_err(WalletError::unavailable)?;
    let _guard = authorized_clients_lock()
        .lock()
        .map_err(|error| WalletError::unavailable(format!("lock NWC clients: {error}")))?;
    let mut clients = load_authorized_clients(&path).map_err(WalletError::unavailable)?;
    clients.default_policy = default.clone();
    store_authorized_clients(&path, &clients).map_err(WalletError::unavailable)?;
    Ok(default_policy_snapshot(&default))
}

pub(super) fn remaining_budget(
    app: &AppHandle,
    owner_pubkey: &str,
    community: &str,
    agent_pubkey: &str,
    timestamp_ms: u64,
) -> Result<u64, String> {
    let path = authorized_clients_path(app, owner_pubkey)?;
    let _guard = authorized_clients_lock()
        .lock()
        .map_err(|error| format!("lock NWC clients: {error}"))?;
    let mut clients = load_authorized_clients(&path)?;
    let key = client_key(community, agent_pubkey);
    let policy = clients.policies.entry(key).or_default();
    let changed = policy.reset_if_elapsed(timestamp_ms);
    let remaining = policy.remaining().unwrap_or_default();
    if changed {
        store_authorized_clients(&path, &clients)?;
    }
    Ok(remaining)
}

fn reserve_budget_at(
    path: &Path,
    community: &str,
    agent_pubkey: &str,
    request_id: &str,
    amount: u64,
    timestamp_ms: u64,
) -> Result<bool, String> {
    let _guard = authorized_clients_lock()
        .lock()
        .map_err(|error| format!("lock NWC clients: {error}"))?;
    let mut clients = load_authorized_clients(path)?;
    let policy = clients
        .policies
        .entry(client_key(community, agent_pubkey))
        .or_default();
    let reset = policy.reset_if_elapsed(timestamp_ms);
    let NwcClientPolicy::Budget {
        amount: budget_amount,
        spent,
        charges,
        ..
    } = policy
    else {
        if reset {
            store_authorized_clients(path, &clients)?;
        }
        return Ok(false);
    };
    if let Some(charge) = charges.get(request_id) {
        if charge.amount() != amount {
            return Err("NWC budget reservation amount changed".to_string());
        }
        return Ok(true);
    }
    if amount > budget_amount.saturating_sub(*spent) {
        if reset {
            store_authorized_clients(path, &clients)?;
        }
        return Ok(false);
    }
    *spent = spent.saturating_add(amount);
    charges.insert(request_id.to_string(), NwcBudgetCharge::pending(amount));
    store_authorized_clients(path, &clients)?;
    Ok(true)
}

pub(super) fn reserve_budget(
    app: &AppHandle,
    owner_pubkey: &str,
    community: &str,
    agent_pubkey: &str,
    request_id: &str,
    amount: u64,
    timestamp_ms: u64,
) -> Result<bool, String> {
    reserve_budget_at(
        &authorized_clients_path(app, owner_pubkey)?,
        community,
        agent_pubkey,
        request_id,
        amount,
        timestamp_ms,
    )
}

pub(super) fn release_budget(
    app: &AppHandle,
    owner_pubkey: &str,
    community: &str,
    agent_pubkey: &str,
    request_id: &str,
) -> Result<(), String> {
    let path = authorized_clients_path(app, owner_pubkey)?;
    let _guard = authorized_clients_lock()
        .lock()
        .map_err(|error| format!("lock NWC clients: {error}"))?;
    let mut clients = load_authorized_clients(&path)?;
    let Some(NwcClientPolicy::Budget { spent, charges, .. }) = clients
        .policies
        .get_mut(&client_key(community, agent_pubkey))
    else {
        return Ok(());
    };
    let Some(charge) = charges.remove(request_id) else {
        return Ok(());
    };
    *spent = spent.saturating_sub(charge.amount());
    store_authorized_clients(&path, &clients)
}

fn settle_budget_at(
    path: &Path,
    community: &str,
    agent_pubkey: &str,
    request_id: &str,
) -> Result<(), String> {
    let _guard = authorized_clients_lock()
        .lock()
        .map_err(|error| format!("lock NWC clients: {error}"))?;
    let mut clients = load_authorized_clients(path)?;
    let Some(NwcClientPolicy::Budget { charges, .. }) = clients
        .policies
        .get_mut(&client_key(community, agent_pubkey))
    else {
        return Ok(());
    };
    let Some(charge) = charges.get_mut(request_id) else {
        return Ok(());
    };
    if !charge.mark_settled() {
        return Ok(());
    }
    store_authorized_clients(path, &clients)
}

pub(super) fn settle_budget(
    app: &AppHandle,
    owner_pubkey: &str,
    community: &str,
    agent_pubkey: &str,
    request_id: &str,
) -> Result<(), String> {
    settle_budget_at(
        &authorized_clients_path(app, owner_pubkey)?,
        community,
        agent_pubkey,
        request_id,
    )
}

mod reconciliation;
pub(super) use reconciliation::reconcile_charge;

#[cfg(test)]
mod rollover_tests;

#[cfg(test)]
mod tests;

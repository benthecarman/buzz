use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{Arc, Weak},
};

use nostr::Keys;
use tokio::sync::{Mutex, OnceCell};
use zeroize::Zeroizing;

use super::{
    lexe_provider::create_lexe_provider, models::WalletError, provider::WalletProvider,
    seed::WalletSeed,
};

type ProviderCell = Arc<OnceCell<Arc<dyn WalletProvider>>>;

/// Creates and caches one wallet provider per active Nostr identity.
///
/// A provider is derived from that identity's secret key and receives a cache
/// directory namespaced by provider and public key. The mutex protects only
/// the in-memory map; it is never held while provider code performs network
/// I/O. Calling `provider_for` again with the same identity reuses the provider.
#[derive(Default)]
pub struct WalletManager {
    providers: Mutex<HashMap<String, ProviderCell>>,
    operation_locks: Mutex<HashMap<String, Weak<Mutex<()>>>>,
}

impl WalletManager {
    /// Returns the provider bound to `keys`, creating it on first use.
    ///
    /// Seed derivation happens only on a cache miss. The raw Nostr secret copy
    /// is zeroized after deriving the provider-neutral wallet seed.
    pub async fn provider_for(
        &self,
        keys: &Keys,
        app_data_dir: &Path,
    ) -> Result<Arc<dyn WalletProvider>, WalletError> {
        let pubkey = keys.public_key().to_hex();
        let cell = self
            .providers
            .lock()
            .await
            .entry(pubkey.clone())
            .or_insert_with(|| Arc::new(OnceCell::new()))
            .clone();
        if let Some(provider) = cell.get() {
            return Ok(Arc::clone(provider));
        }
        let secret = Zeroizing::new(keys.secret_key().to_secret_bytes());
        let seed = WalletSeed::derive(&secret)?;
        drop(secret);
        let provider_dir = app_data_dir.join("wallet").join("lexe").join(pubkey);
        cell.get_or_try_init(|| initialize_provider(seed, provider_dir))
            .await
            .map(Arc::clone)
    }

    /// Serialize a durable payment attempt across windows and concurrent
    /// command invokes. Weak entries avoid retaining one lock per historical
    /// request forever.
    pub async fn operation_lock(&self, payer_pubkey: &str, request_id: &str) -> Arc<Mutex<()>> {
        let key = format!("{payer_pubkey}:{request_id}");
        let mut locks = self.operation_locks.lock().await;
        if let Some(lock) = locks.get(&key).and_then(Weak::upgrade) {
            return lock;
        }
        locks.retain(|_, lock| lock.strong_count() > 0);
        let lock = Arc::new(Mutex::new(()));
        locks.insert(key, Arc::downgrade(&lock));
        lock
    }

    /// Derives the wallet recovery phrase without persisting secret material.
    pub fn recovery_phrase(&self, keys: &Keys) -> Result<Zeroizing<String>, WalletError> {
        let secret = Zeroizing::new(keys.secret_key().to_secret_bytes());
        let seed = WalletSeed::derive(&secret)?;
        drop(secret);
        seed.mnemonic()
    }
}

async fn initialize_provider(
    seed: WalletSeed,
    provider_dir: PathBuf,
) -> Result<Arc<dyn WalletProvider>, WalletError> {
    tokio::task::spawn_blocking(move || create_lexe_provider(seed, &provider_dir))
        .await
        .map_err(|error| WalletError::unavailable(format!("initialize wallet task: {error}")))?
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn operation_lock_is_shared_only_for_the_same_attempt() {
        let manager = WalletManager::default();

        let first = manager.operation_lock("payer", "request").await;
        let same = manager.operation_lock("payer", "request").await;
        let different_request = manager.operation_lock("payer", "other").await;
        let different_payer = manager.operation_lock("other", "request").await;

        assert!(Arc::ptr_eq(&first, &same));
        assert!(!Arc::ptr_eq(&first, &different_request));
        assert!(!Arc::ptr_eq(&first, &different_payer));
    }
}

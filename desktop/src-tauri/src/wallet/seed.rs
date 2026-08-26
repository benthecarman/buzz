use bip39::Mnemonic;
use hkdf::Hkdf;
use sha2_10::Sha256;
use zeroize::Zeroizing;

use super::models::WalletError;

const DERIVATION_SALT: &[u8] = b"buzz.wallet.seed";
const DERIVATION_INFO: &[u8] = b"v1/bip39-entropy";

/// Provider-neutral wallet seed derived from the raw Nostr secret key.
///
/// The derivation labels and output size are a recovery compatibility
/// contract. Providers may convert the bytes internally but must not alter the
/// derivation.
#[derive(Clone)]
pub struct WalletSeed(Zeroizing<[u8; 32]>);

impl WalletSeed {
    /// Derive a 32-byte wallet seed from a raw 32-byte Nostr secret key.
    pub fn derive(nostr_secret: &[u8; 32]) -> Result<Self, WalletError> {
        let hkdf = Hkdf::<Sha256>::new(Some(DERIVATION_SALT), nostr_secret);
        let mut entropy = Zeroizing::new([0_u8; 32]);
        hkdf.expand(DERIVATION_INFO, entropy.as_mut())
            .map_err(|_| WalletError::unavailable("wallet seed derivation failed"))?;
        Ok(Self(entropy))
    }

    /// Borrow the provider-neutral 32-byte seed.
    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }

    /// Encode the seed as its 24-word English BIP39 recovery phrase.
    pub fn mnemonic(&self) -> Result<Zeroizing<String>, WalletError> {
        Mnemonic::from_entropy(self.0.as_ref())
            .map(|mnemonic| Zeroizing::new(mnemonic.to_string()))
            .map_err(|error| WalletError::unavailable(format!("encode recovery phrase: {error}")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn derivation_is_deterministic_and_domain_separated() {
        let nsec = [0x42; 32];
        let first = WalletSeed::derive(&nsec).unwrap();
        let second = WalletSeed::derive(&nsec).unwrap();
        assert_eq!(first.as_bytes(), second.as_bytes());
        assert_ne!(first.as_bytes(), &nsec);
        assert_ne!(
            WalletSeed::derive(&[0x43; 32]).unwrap().as_bytes(),
            first.as_bytes()
        );
    }

    #[test]
    fn frozen_derivation_vector() {
        let seed = WalletSeed::derive(&[0x42; 32]).unwrap();
        assert_eq!(
            hex::encode(seed.as_bytes()),
            "3367603e5de18394ca8efe77be0bdc84b5d7d757de8752a371feecfff60352d2"
        );
        assert_eq!(
            seed.mnemonic().unwrap().as_str(),
            "cricket deposit auto rookie blouse skill clay thank jeans utility warm annual \
             frost two garage special famous breeze leisure supreme youth accuse ensure frame"
        );
    }
}

//! Self-custody wallet: keypair generation, address derivation, and lookup
//! for signing.

use std::path::Path;

use tw_chain::crypto::sign_ed25519 as sign;
use tw_chain::utils::transaction_utils::construct_address_for;

use crate::error::Result;
use crate::wallet::keystore::{KeyStore, StoredKeypair};
use crate::wallet::ADDRESS_VERSION;

/// A wallet backed by an encrypted keystore file.
pub struct Wallet {
    store: KeyStore,
}

impl Wallet {
    /// Create a fresh wallet at `path`, protected by `passphrase`.
    pub fn create(path: &Path, passphrase: &str) -> Result<Wallet> {
        Ok(Wallet {
            store: KeyStore::create(path, passphrase)?,
        })
    }

    /// Open an existing wallet, decrypting its entries with `passphrase`.
    pub fn open(path: &Path, passphrase: &str) -> Result<Wallet> {
        Ok(Wallet {
            store: KeyStore::open(path, passphrase)?,
        })
    }

    /// Generate a new keypair, derive its address, persist it to the
    /// keystore, and return the address.
    pub fn new_address(&mut self) -> Result<String> {
        let (pk, sk) = sign::gen_keypair();
        let address = construct_address_for(&pk, Some(ADDRESS_VERSION));

        self.store.insert(StoredKeypair {
            public_key: hex::encode(pk.as_ref()),
            secret_key: hex::encode(sk.as_ref()),
            address: address.clone(),
        })?;

        Ok(address)
    }

    /// All addresses currently held in the wallet.
    pub fn addresses(&self) -> Vec<String> {
        self.store
            .entries()
            .iter()
            .map(|kp| kp.address.clone())
            .collect()
    }

    /// Look up the keypair for `address`, decoding its hex-encoded key
    /// material back into typed `tw_chain` keys.
    pub(crate) fn key_for(&self, address: &str) -> Option<(sign::PublicKey, sign::SecretKey)> {
        let kp = self
            .store
            .entries()
            .iter()
            .find(|kp| kp.address == address)?;

        let pk_bytes = hex::decode(&kp.public_key).ok()?;
        let sk_bytes = hex::decode(&kp.secret_key).ok()?;

        let pk = sign::PublicKey::from_slice(&pk_bytes)?;
        let sk = sign::SecretKey::from_slice(&sk_bytes)?;

        Some((pk, sk))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_address_is_deterministic_length_and_persists() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("w.json");
        let addr = {
            let mut w = Wallet::create(&path, "pw").unwrap();
            let a = w.new_address().unwrap();
            assert_eq!(a.len(), 64); // 32-byte hash, hex
            a
        };
        let w = Wallet::open(&path, "pw").unwrap();
        assert!(w.addresses().contains(&addr));
        assert!(w.key_for(&addr).is_some());
    }
}

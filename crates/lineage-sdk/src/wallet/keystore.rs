//! Encrypted keystore file: a master key (unlocked by the passphrase) encrypts
//! each stored keypair independently, so the passphrase can be rotated without
//! re-encrypting every entry.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};
use crate::wallet::crypto::{decrypt, encrypt, make_key, pwhash, secretbox};

/// A single keypair as persisted in the keystore, hex-encoded.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct StoredKeypair {
    pub public_key: String,
    pub secret_key: String,
    pub address: String,
}

/// On-disk representation of the keystore file.
#[derive(Debug, Serialize, Deserialize)]
struct KeyStoreFile {
    salt: String,
    enc_master_key: String,
    addresses: BTreeMap<String, String>,
}

/// A file-based encrypted keystore. The passphrase unlocks a master key, which
/// in turn decrypts each stored keypair.
pub struct KeyStore {
    path: PathBuf,
    salt: pwhash::Salt,
    enc_master_key: Vec<u8>,
    master: secretbox::Key,
    entries: Vec<StoredKeypair>,
}

impl KeyStore {
    /// Create a fresh keystore at `path`, protected by `passphrase`.
    pub fn create(path: &Path, passphrase: &str) -> Result<KeyStore> {
        let master = secretbox::gen_key();
        let salt = pwhash::gen_salt();
        let pass_key = make_key(passphrase.as_bytes(), &salt)?;
        let enc_master_key = encrypt(master.as_ref(), &pass_key);

        let store = KeyStore {
            path: path.to_path_buf(),
            salt,
            enc_master_key,
            master,
            entries: Vec::new(),
        };
        store.write()?;
        Ok(store)
    }

    /// Open an existing keystore, decrypting the master key and all entries
    /// with `passphrase`.
    pub fn open(path: &Path, passphrase: &str) -> Result<KeyStore> {
        let bytes = std::fs::read(path)
            .map_err(|e| Error::Keystore(format!("failed to read keystore: {e}")))?;
        let file: KeyStoreFile = serde_json::from_slice(&bytes)
            .map_err(|e| Error::Keystore(format!("failed to parse keystore: {e}")))?;

        let salt_bytes = hex::decode(&file.salt)
            .map_err(|e| Error::Keystore(format!("bad salt encoding: {e}")))?;
        let salt = pwhash::Salt::from_slice(&salt_bytes)
            .ok_or_else(|| Error::Keystore("bad salt length".into()))?;

        let pass_key = make_key(passphrase.as_bytes(), &salt)?;

        let enc_master = hex::decode(&file.enc_master_key)
            .map_err(|e| Error::Keystore(format!("bad master key encoding: {e}")))?;
        let master_bytes = decrypt(&enc_master, &pass_key)?;
        let master = secretbox::Key::from_slice(&master_bytes)
            .ok_or_else(|| Error::Keystore("bad master key length".into()))?;

        let mut entries = Vec::with_capacity(file.addresses.len());
        for (address, hex_blob) in &file.addresses {
            let blob = hex::decode(hex_blob)
                .map_err(|e| Error::Keystore(format!("bad entry encoding: {e}")))?;
            let plain = decrypt(&blob, &master)?;
            let kp: StoredKeypair = serde_json::from_slice(&plain)
                .map_err(|e| Error::Keystore(format!("bad entry contents: {e}")))?;
            if kp.address != *address {
                return Err(Error::Keystore("entry address mismatch".into()));
            }
            entries.push(kp);
        }

        Ok(KeyStore {
            path: path.to_path_buf(),
            salt,
            enc_master_key: enc_master,
            master,
            entries,
        })
    }

    /// Insert a keypair, encrypting it with the master key and persisting the
    /// keystore to disk.
    pub fn insert(&mut self, kp: StoredKeypair) -> Result<()> {
        self.entries.push(kp);
        self.write()
    }

    /// All decrypted entries currently held in memory.
    pub fn entries(&self) -> &[StoredKeypair] {
        &self.entries
    }

    /// Re-encrypt all entries with the master key and write the keystore file
    /// atomically.
    fn write(&self) -> Result<()> {
        let mut addresses = BTreeMap::new();
        for kp in &self.entries {
            let plain = serde_json::to_vec(kp)
                .map_err(|e| Error::Keystore(format!("failed to encode entry: {e}")))?;
            let blob = encrypt(&plain, &self.master);
            addresses.insert(kp.address.clone(), hex::encode(blob));
        }

        let file = KeyStoreFile {
            salt: hex::encode(self.salt.as_ref()),
            enc_master_key: hex::encode(&self.enc_master_key),
            addresses,
        };

        let json = serde_json::to_vec_pretty(&file)
            .map_err(|e| Error::Keystore(format!("failed to encode keystore: {e}")))?;

        write_atomic(&self.path, &json)
    }
}

fn write_atomic(path: &Path, contents: &[u8]) -> Result<()> {
    let tmp_path = path.with_extension("tmp");
    std::fs::write(&tmp_path, contents)
        .map_err(|e| Error::Keystore(format!("failed to write keystore: {e}")))?;
    std::fs::rename(&tmp_path, path)
        .map_err(|e| Error::Keystore(format!("failed to finalize keystore: {e}")))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn create_then_open_round_trips_entries() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("wallet.json");
        {
            let mut ks = KeyStore::create(&path, "pw").unwrap();
            ks.insert(StoredKeypair {
                public_key: "aa".into(),
                secret_key: "bb".into(),
                address: "addr1".into(),
            })
            .unwrap();
        }
        let ks = KeyStore::open(&path, "pw").unwrap();
        assert_eq!(ks.entries().len(), 1);
        assert_eq!(ks.entries()[0].address, "addr1");
        assert_eq!(ks.entries()[0].secret_key, "bb");
    }

    #[test]
    fn open_with_wrong_passphrase_fails() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("wallet.json");
        KeyStore::create(&path, "right").unwrap();
        assert!(KeyStore::open(&path, "wrong").is_err());
    }
}

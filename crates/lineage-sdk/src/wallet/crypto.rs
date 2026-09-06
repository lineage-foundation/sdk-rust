//! Passphrase-derived encryption, ported from fleet-wallet's tw_chain usage.

pub use tw_chain::crypto::{pbkdf2 as pwhash, secretbox_chacha20_poly1305 as secretbox};

use crate::error::{Error, Result};

pub fn make_key(passphrase: &[u8], salt: &pwhash::Salt) -> Result<secretbox::Key> {
    let mut kb = [0u8; secretbox::KEY_LEN];
    pwhash::derive_key(&mut kb, passphrase, salt, pwhash::OPSLIMIT_INTERACTIVE);
    secretbox::Key::from_slice(&kb).ok_or_else(|| Error::Keystore("bad key length".into()))
}

pub fn encrypt(plain: &[u8], key: &secretbox::Key) -> Vec<u8> {
    let nonce = secretbox::gen_nonce();
    let mut out = nonce.as_ref().to_vec();
    out.extend_from_slice(&secretbox::seal(plain.to_vec(), &nonce, key).unwrap_or_default());
    out
}

pub fn decrypt(blob: &[u8], key: &secretbox::Key) -> Result<Vec<u8>> {
    if blob.len() < secretbox::NONCE_LEN {
        return Err(Error::Keystore("ciphertext too short".into()));
    }
    let (nonce_bytes, cipher) = blob.split_at(secretbox::NONCE_LEN);
    let nonce = secretbox::Nonce::from_slice(nonce_bytes)
        .ok_or_else(|| Error::Keystore("bad nonce".into()))?;
    secretbox::open(cipher.to_vec(), &nonce, key)
        .ok_or_else(|| Error::Keystore("wrong passphrase or corrupt data".into()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_under_correct_passphrase() {
        let salt = pwhash::gen_salt();
        let key = make_key(b"hunter2", &salt).unwrap();
        let blob = encrypt(b"secret bytes", &key);
        assert_eq!(decrypt(&blob, &key).unwrap(), b"secret bytes");
    }

    #[test]
    fn wrong_passphrase_fails_to_decrypt() {
        let salt = pwhash::gen_salt();
        let good = make_key(b"hunter2", &salt).unwrap();
        let bad = make_key(b"nope", &salt).unwrap();
        let blob = encrypt(b"secret", &good);
        assert!(decrypt(&blob, &bad).is_err());
    }
}

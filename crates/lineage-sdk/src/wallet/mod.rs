//! Self-custody wallet: encrypted keystore, key management, and signing.

pub mod crypto;
pub mod keystore;
pub mod account;

pub use account::Wallet;

/// The network address version used to derive and validate addresses.
pub const ADDRESS_VERSION: u64 = 6;

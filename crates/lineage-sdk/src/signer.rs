//! Transaction signing backends: sign locally or delegate to a node.

use crate::client::Client;
use crate::error::{Error, Result};
use crate::wallet::wallet::Wallet;

/// Result of a successfully submitted payment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Receipt {
    pub tx_hash: String,
    pub to_address: String,
    pub amount: u64,
}

/// A backend capable of paying an address from wallet-held funds.
pub trait Signer {
    async fn pay(&self, to: &str, amount: u64) -> Result<Receipt>;
}

/// Signs transactions locally using keys held in a [`Wallet`], then submits
/// them to the mempool via [`Client::submit_transactions`].
///
/// UTXO selection and change handling are implemented in a later change;
/// for now `pay` is a compiling placeholder.
pub struct LocalSigner<'a> {
    client: &'a Client,
    wallet: &'a Wallet,
    change_address: String,
}

impl<'a> LocalSigner<'a> {
    pub fn new(client: &'a Client, wallet: &'a Wallet, change_address: impl Into<String>) -> Self {
        LocalSigner {
            client,
            wallet,
            change_address: change_address.into(),
        }
    }
}

impl<'a> Signer for LocalSigner<'a> {
    async fn pay(&self, _to: &str, _amount: u64) -> Result<Receipt> {
        let _ = (self.client, self.wallet, &self.change_address);
        Err(Error::Keystore("not yet implemented".into()))
    }
}

//! Transaction signing backends: sign locally or delegate to a node.

use crate::client::Client;
use crate::error::{Error, Result};
use crate::tx::{build_signed_payment, PayOutput, SpendInput};
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
    async fn pay(&self, to: &str, amount: u64) -> Result<Receipt> {
        let addresses = self.wallet.addresses();
        let address_refs: Vec<&str> = addresses.iter().map(String::as_str).collect();
        let balances = self.client.balances(&address_refs).await?;

        let mut selected = Vec::new();
        let mut total: u64 = 0;

        'outer: for (address, utxos) in &balances.balance.address_list {
            let Some((public_key, secret_key)) = self.wallet.key_for(address) else {
                continue;
            };
            for utxo in utxos {
                let Some(value) = utxo.value.get("Token").and_then(|v| v.as_u64()) else {
                    continue;
                };
                selected.push(SpendInput {
                    t_hash: utxo.out_point.t_hash.clone(),
                    n: utxo.out_point.n as i32,
                    public_key,
                    secret_key: secret_key.clone(),
                });
                total += value;
                if total >= amount {
                    break 'outer;
                }
            }
        }

        if total < amount {
            return Err(Error::Keystore("insufficient funds".into()));
        }

        let mut outputs = vec![PayOutput {
            address: to.to_string(),
            amount,
        }];
        let change = total - amount;
        if change > 0 {
            outputs.push(PayOutput {
                address: self.change_address.clone(),
                amount: change,
            });
        }

        let (tx_hash, tx) = build_signed_payment(&selected, &outputs);
        let dto = serde_json::to_value(&tx)?;
        self.client.submit_transactions(&[dto]).await?;

        Ok(Receipt {
            tx_hash,
            to_address: to.to_string(),
            amount,
        })
    }
}

/// Delegates signing to a node's wallet via `POST /v1/payments`.
pub struct NodeSigner<'a> {
    client: &'a Client,
    passphrase: String,
}

impl<'a> NodeSigner<'a> {
    pub fn new(client: &'a Client, passphrase: impl Into<String>) -> Self {
        NodeSigner {
            client,
            passphrase: passphrase.into(),
        }
    }
}

impl<'a> Signer for NodeSigner<'a> {
    async fn pay(&self, to: &str, amount: u64) -> Result<Receipt> {
        let accepted = self
            .client
            .make_payment("address", to, amount, &self.passphrase, None)
            .await?;

        Ok(Receipt {
            tx_hash: accepted.tx_hash.unwrap_or_default(),
            to_address: to.to_string(),
            amount,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::client::Hosts;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn client_for(server: &MockServer) -> Client {
        Client::new(Hosts {
            mempool: server.uri(),
            storage: server.uri(),
            miner: server.uri(),
        })
        .unwrap()
    }

    #[tokio::test]
    async fn pay_selects_utxos_and_submits_signed_transaction() {
        let dir = tempfile::tempdir().unwrap();
        let path_buf = dir.path().join("w.json");
        let mut wallet = Wallet::create(&path_buf, "pw").unwrap();
        let address = wallet.new_address().unwrap();

        let mut address_list = serde_json::Map::new();
        address_list.insert(
            address.clone(),
            serde_json::json!([
                {"out_point": {"n": 0, "t_hash": "g0000"}, "value": {"Token": 2000}}
            ]),
        );
        let balances_body = serde_json::json!({
            "balance": {
                "address_list": address_list,
                "total": {"tokens": 2000, "items": {}}
            }
        });

        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/v1/balances"))
            .respond_with(ResponseTemplate::new(200).set_body_json(balances_body))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/v1/transactions"))
            .respond_with(ResponseTemplate::new(201).set_body_json(serde_json::json!({"transactions": {}})))
            .mount(&server)
            .await;

        let client = client_for(&server);
        let signer = LocalSigner::new(&client, &wallet, "change-address");

        let receipt = signer.pay("recipient-address", 1000).await.unwrap();

        assert_eq!(receipt.to_address, "recipient-address");
        assert_eq!(receipt.amount, 1000);
        assert!(!receipt.tx_hash.is_empty());
    }

    #[tokio::test]
    async fn node_signer_pay_posts_payment_and_returns_receipt() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/payments"))
            .respond_with(ResponseTemplate::new(202).set_body_json(serde_json::json!({
                "to_address": "recipient-address",
                "amount": {"kind": "token", "amount": 1000},
                "tx_hash": "g.."
            })))
            .mount(&server)
            .await;

        let client = client_for(&server);
        let signer = NodeSigner::new(&client, "pw");

        let receipt = signer.pay("recipient-address", 1000).await.unwrap();

        assert_eq!(receipt.tx_hash, "g..");
        assert_eq!(receipt.to_address, "recipient-address");
        assert_eq!(receipt.amount, 1000);
    }
}

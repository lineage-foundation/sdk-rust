//! Transaction signing backends: sign locally or delegate to a node.

use std::collections::{HashMap, HashSet};

use tw_chain::crypto::sha3_256;
use tw_chain::crypto::sign_ed25519 as sign;
use tw_chain::primitives::asset::Asset;

use crate::client::Client;
use crate::druid::{construct_tx_ins_address, create_2w_tx_half, generate_druid, KeyPairs};
use crate::error::{Error, Result};
use crate::models::{DruidExpectation, Pending2WTxDetails, Pending2WTxStatus, PendingHalf};
use crate::tx::{build_signed_payment, PayOutput, SpendInput};
use crate::valence::ValenceClient;
use crate::wallet::Wallet;

/// Result of a successfully submitted payment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Receipt {
    pub tx_hash: String,
    pub to_address: String,
    pub amount: u64,
}

/// A payment that has been built and signed but not yet submitted, for
/// inspection or a dry run.
#[derive(Debug, Clone)]
pub struct PreparedPayment {
    pub tx_hash: String,
    pub to_address: String,
    pub amount: u64,
    pub change: u64,
    pub inputs: usize,
    /// The `/v1/transactions` payload that [`LocalSigner::pay`] would submit.
    pub transaction: serde_json::Value,
}

/// A backend capable of paying an address from wallet-held funds.
///
/// The `pay` future is not `Send`-bounded; the SDK is used from a single async
/// context (the CLI), not spawned across threads.
#[allow(async_fn_in_trait)]
pub trait Signer {
    async fn pay(&self, to: &str, amount: u64) -> Result<Receipt>;
}

/// Signs transactions locally using keys held in a [`Wallet`], then submits
/// them to the mempool via [`Client::submit_transactions`].
pub struct LocalSigner<'a> {
    client: &'a Client,
    wallet: &'a Wallet,
    change_address: String,
    valence_host: Option<String>,
}

impl<'a> LocalSigner<'a> {
    pub fn new(client: &'a Client, wallet: &'a Wallet, change_address: impl Into<String>) -> Self {
        LocalSigner {
            client,
            wallet,
            change_address: change_address.into(),
            valence_host: None,
        }
    }

    /// Configures the valence mailbox host used by the two-way payment
    /// methods (`make_2way_payment` and friends). Two-way payments are
    /// unusable until this is set.
    pub fn with_valence_host(mut self, host: impl Into<String>) -> Self {
        self.valence_host = Some(host.into());
        self
    }

    fn valence(&self) -> Result<ValenceClient> {
        let host = self
            .valence_host
            .clone()
            .ok_or_else(|| Error::Tx("valence host not configured".into()))?;
        ValenceClient::new(host)
    }

    fn keypairs_for(&self, addresses: &[String]) -> Result<KeyPairs> {
        let mut keypairs = KeyPairs::new();
        for address in addresses {
            let kp = self
                .wallet
                .key_for(address)
                .ok_or_else(|| Error::Tx(format!("no keypair for address {address}")))?;
            keypairs.insert(address.clone(), kp);
        }
        Ok(keypairs)
    }

    /// Select inputs, build, and sign a payment without submitting it.
    pub async fn prepare(&self, to: &str, amount: u64) -> Result<PreparedPayment> {
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
        let transaction = serde_json::to_value(&tx)?;
        Ok(PreparedPayment {
            tx_hash,
            to_address: to.to_string(),
            amount,
            change,
            inputs: selected.len(),
            transaction,
        })
    }

    /// Mints `amount` of a new item asset against `address`: signs the
    /// item-asset signable hash -- `hex(sha3_256("Item:{amount}"))`, itself
    /// signed as its UTF-8 hex text, exactly as per-input payment signatures
    /// are (see [`crate::druid::create_2w_tx_half`]) -- with `address`'s
    /// keypair, and submits `POST /v1/items`. `default_genesis_hash` selects
    /// the node's well-known default item DRS transaction hash rather than
    /// having one freshly assigned. Mirrors sdk-go's `Wallet.CreateItems`.
    pub async fn create_items(
        &self,
        address: &str,
        default_genesis_hash: bool,
        amount: u64,
        metadata: Option<String>,
    ) -> Result<serde_json::Value> {
        let (public_key, secret_key) = self
            .wallet
            .key_for(address)
            .ok_or_else(|| Error::Tx(format!("no keypair for address {address}")))?;

        let signable_hash = hex::encode(sha3_256::digest(format!("Item:{amount}").as_bytes()));
        let signature = sign::sign_detached(signable_hash.as_bytes(), &secret_key);

        let body = serde_json::json!({
            "item_amount": amount,
            "script_public_key": address,
            "public_key": hex::encode(public_key.as_ref()),
            "signature": hex::encode(signature.as_ref()),
            "genesis_hash_spec": if default_genesis_hash { "Default" } else { "Create" },
            "metadata": metadata,
        });

        self.client.post_items(body).await
    }

    /* ---------------------------------------------------------------- */
    /*                          Two-way payments                         */
    /* ---------------------------------------------------------------- */

    /// Offers a two-way (DRUID) trade to `payment_address`: this wallet will
    /// pay `sending_asset` to `payment_address` in exchange for
    /// `receiving_asset` delivered to `receive_keypair`'s address. Builds
    /// this party's transaction half (sourcing inputs from `all_keypairs`'s
    /// addresses, change back to `receive_keypair`), posts the plaintext
    /// offer to valence (addressed to `payment_address`'s mailbox, signed by
    /// `receive_keypair`), and returns a [`PendingHalf`] -- this party's
    /// half, sealed at rest under the wallet's keystore -- for the caller to
    /// persist until [`LocalSigner::fetch_pending_2way_payment`] reports it
    /// settled. Mirrors sdk-go's `Wallet.Make2WayPayment`.
    pub async fn make_2way_payment(
        &self,
        payment_address: &str,
        sending_asset: Asset,
        receiving_asset: Asset,
        all_keypairs: &[String],
        receive_keypair: &str,
    ) -> Result<PendingHalf> {
        if all_keypairs.is_empty() {
            return Err(Error::Tx("no keypairs provided".into()));
        }

        let (sender_pk, sender_sk) = self
            .wallet
            .key_for(receive_keypair)
            .ok_or_else(|| Error::Tx(format!("no keypair for address {receive_keypair}")))?;

        let keypairs = self.keypairs_for(all_keypairs)?;
        let address_refs: Vec<&str> = all_keypairs.iter().map(String::as_str).collect();
        let balance = self.client.balances_ordered(&address_refs).await?;

        let druid = generate_druid();
        // senderExpectation: what this (sending) party expects to receive.
        // receiverExpectation: what the counterparty (payee) is owed by
        // this half.
        let sender_expectation = DruidExpectation {
            from: String::new(),
            to: receive_keypair.to_string(),
            asset: receiving_asset,
        };
        let mut receiver_expectation = DruidExpectation {
            from: String::new(),
            to: payment_address.to_string(),
            asset: sending_asset,
        };

        let my_half = create_2w_tx_half(
            &druid,
            sender_expectation.clone(),
            receiver_expectation.clone(),
            &balance,
            &keypairs,
            receive_keypair,
            0,
        )?;

        // Now that this half's inputs are known, fill in the "from" the
        // counterparty will use to correlate their acceptance transaction.
        receiver_expectation.from = construct_tx_ins_address(&my_half.inputs)?;

        let encrypted_half = self.wallet.encrypt_transaction(&my_half)?;

        let details = Pending2WTxDetails {
            druid: druid.clone(),
            sender_expectation: sender_expectation.clone(),
            receiver_expectation: receiver_expectation.clone(),
            status: Pending2WTxStatus::Pending,
            mempool_host: self.client.mempool_host().to_string(),
        };

        self.valence()?.post(payment_address, &sender_pk, &sender_sk, &details).await?;

        Ok(PendingHalf {
            druid,
            encrypted_half,
            sender_expectation,
            receiver_expectation,
        })
    }

    /// Polls this wallet's own mailboxes -- one per address in
    /// `all_keypairs`, deduplicated -- and does both of this wallet's
    /// possible roles in a two-way (DRUID) trade against whatever it finds
    /// there:
    ///
    /// 1. Acceptor discovery: an offer `make_2way_payment` posts is
    ///    addressed to whichever of the counterparty's own addresses it was
    ///    handed as `payment_address`, so it lands in one of *our* mailboxes
    ///    here. Any mailbox entry whose druid isn't in `stored` -- this
    ///    wallet never initiated it -- is surfaced as-is in the returned
    ///    `pending` map.
    /// 2. Initiator settlement: an offer this wallet made shows up back in
    ///    its own mailbox once the counterparty accepts. For any such
    ///    entry -- druid present in `stored`, status accepted -- the stored
    ///    half is decrypted, its DRUID expectation is replaced with the
    ///    counterparty-filled `sender_expectation` now on the mailbox entry,
    ///    the resulting transaction is submitted to this wallet's own
    ///    mempool, and the settled mailbox entry is deleted.
    ///
    /// A failure against one mailbox, or one mailbox entry, never discards
    /// progress already made against the others: `pending` and `settled`
    /// are always returned in full, alongside every error encountered along
    /// the way (joined into a single [`Error::Multiple`] when non-empty),
    /// mirroring sdk-go's `errors.Join`-based
    /// `Wallet.FetchPending2WayPayment`.
    pub async fn fetch_pending_2way_payment(
        &self,
        stored: &[PendingHalf],
        all_keypairs: &[String],
    ) -> (HashMap<String, Pending2WTxDetails>, Vec<String>, Option<Error>) {
        let mut pending = HashMap::new();
        let mut settled = Vec::new();
        let mut errors = Vec::new();

        let valence = match self.valence() {
            Ok(v) => v,
            Err(e) => return (pending, settled, Some(e)),
        };

        let stored_by_druid: HashMap<&str, &PendingHalf> =
            stored.iter().map(|half| (half.druid.as_str(), half)).collect();

        let mut seen = HashSet::new();
        for address in all_keypairs {
            if !seen.insert(address.as_str()) {
                continue;
            }

            let Some((public_key, secret_key)) = self.wallet.key_for(address) else {
                errors.push(Error::Tx(format!("no keypair for address {address}")));
                continue;
            };

            let entries = match valence.get(address, &public_key, &secret_key).await {
                Ok(entries) => entries,
                Err(e) => {
                    // This mailbox is unreachable; skip it and keep polling
                    // the rest rather than aborting discovery/settlement
                    // entirely.
                    errors.push(e);
                    continue;
                }
            };

            for (druid, details) in entries {
                let Some(half) = stored_by_druid.get(druid.as_str()) else {
                    // An incoming offer (or status update) this wallet
                    // never initiated: surface it as pending.
                    pending.insert(druid, details);
                    continue;
                };
                if details.status != Pending2WTxStatus::Accepted {
                    // One of our own offers that isn't settled yet.
                    pending.insert(druid, details);
                    continue;
                }

                let mut tx = match self.wallet.decrypt_transaction(&half.encrypted_half) {
                    Ok(tx) => tx,
                    Err(e) => {
                        errors.push(e);
                        continue;
                    }
                };
                if tx.druid_info.expectations.is_empty() {
                    errors.push(Error::Tx(format!(
                        "stored half for druid {druid} has no DRUID expectations"
                    )));
                    continue;
                }
                // The counterparty has now filled in sender_expectation.from;
                // replace our stored (incomplete) expectation with theirs.
                tx.druid_info.expectations[0] = details.sender_expectation.clone();

                let submission = match tx.to_submission_value() {
                    Ok(v) => v,
                    Err(e) => {
                        errors.push(e);
                        continue;
                    }
                };
                if let Err(e) = self.client.submit_transactions(&[submission]).await {
                    errors.push(e);
                    continue;
                }

                if let Err(e) = valence.delete(&druid, address, &public_key, &secret_key).await {
                    // The half is already submitted on-chain even though
                    // the valence entry couldn't be cleaned up -- this is
                    // committed progress and must still be reported
                    // settled.
                    errors.push(e);
                    settled.push(druid);
                    continue;
                }
                settled.push(druid);
            }
        }

        let error = if errors.is_empty() { None } else { Some(Error::Multiple(errors)) };
        (pending, settled, error)
    }

    /// Shared implementation behind `accept_2way_payment` and
    /// `reject_2way_payment`: stamps `details` with `status`, and -- only
    /// when accepting -- builds this party's matching transaction half
    /// (paying `details.sender_expectation`'s asset to its address,
    /// embedding `details.receiver_expectation` as this party's own DRUID
    /// expectation) and submits it to `details.mempool_host`, before posting
    /// the updated status back to valence (addressed to
    /// `details.sender_expectation.to`'s mailbox, signed by this party's own
    /// -- `details.receiver_expectation.to` -- keypair). Mirrors sdk-go's
    /// `Wallet.handle2WTxResponse`.
    async fn handle_2way_response(
        &self,
        mut details: Pending2WTxDetails,
        status: Pending2WTxStatus,
        all_keypairs: &[String],
    ) -> Result<()> {
        let keypairs = self.keypairs_for(all_keypairs)?;
        let (receiver_pk, receiver_sk) = keypairs
            .get(&details.receiver_expectation.to)
            .cloned()
            .ok_or_else(|| {
                Error::Tx(format!(
                    "no keypair for receiver address {}",
                    details.receiver_expectation.to
                ))
            })?;

        details.status = status;

        if status == Pending2WTxStatus::Accepted {
            let address_refs: Vec<&str> = all_keypairs.iter().map(String::as_str).collect();
            let balance = self.client.balances_ordered(&address_refs).await?;

            let my_half = create_2w_tx_half(
                &details.druid,
                details.receiver_expectation.clone(),
                details.sender_expectation.clone(),
                &balance,
                &keypairs,
                &details.receiver_expectation.to,
                0,
            )?;

            details.sender_expectation.from = construct_tx_ins_address(&my_half.inputs)?;

            let submission = my_half.to_submission_value()?;
            self.client.submit_transactions_to(&details.mempool_host, &[submission]).await?;
        }

        self.valence()?
            .post(&details.sender_expectation.to, &receiver_pk, &receiver_sk, &details)
            .await?;
        Ok(())
    }

    /// Accepts a pending two-way trade offer described by `details`: pays
    /// `details.sender_expectation`'s asset to the offering party, embeds
    /// this party's own `details.receiver_expectation` as its half of the
    /// DRUID trade, submits the resulting transaction to
    /// `details.mempool_host`, and posts the accepted status (with
    /// `sender_expectation.from` now filled in) back to valence.
    /// `all_keypairs` must include the keypair for
    /// `details.receiver_expectation.to` (this party's own address in the
    /// offer). Mirrors sdk-go's `Wallet.Accept2WayPayment`.
    pub async fn accept_2way_payment(&self, details: Pending2WTxDetails, all_keypairs: &[String]) -> Result<()> {
        self.handle_2way_response(details, Pending2WTxStatus::Accepted, all_keypairs).await
    }

    /// Declines a pending two-way trade offer described by `details`: no
    /// transaction is built or submitted, but the rejected status is posted
    /// back to valence so the offering party's `fetch_pending_2way_payment`
    /// can observe it. `all_keypairs` must include the keypair for
    /// `details.receiver_expectation.to`. Mirrors sdk-go's
    /// `Wallet.Reject2WayPayment`.
    pub async fn reject_2way_payment(&self, details: Pending2WTxDetails, all_keypairs: &[String]) -> Result<()> {
        self.handle_2way_response(details, Pending2WTxStatus::Rejected, all_keypairs).await
    }
}

impl<'a> Signer for LocalSigner<'a> {
    async fn pay(&self, to: &str, amount: u64) -> Result<Receipt> {
        let prepared = self.prepare(to, amount).await?;
        self.client
            .submit_transactions(&[prepared.transaction])
            .await?;
        Ok(Receipt {
            tx_hash: prepared.tx_hash,
            to_address: prepared.to_address,
            amount: prepared.amount,
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
    async fn prepare_builds_without_submitting() {
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
            "balance": { "address_list": address_list, "total": {"tokens": 2000, "items": {}} }
        });

        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/v1/balances"))
            .respond_with(ResponseTemplate::new(200).set_body_json(balances_body))
            .mount(&server)
            .await;
        // Deliberately no /v1/transactions mock: prepare must not submit.

        let client = client_for(&server);
        let signer = LocalSigner::new(&client, &wallet, "change-address");
        let prepared = signer.prepare("recipient-address", 1000).await.unwrap();

        assert_eq!(prepared.to_address, "recipient-address");
        assert_eq!(prepared.amount, 1000);
        assert_eq!(prepared.change, 1000);
        assert_eq!(prepared.inputs, 1);
        assert!(!prepared.tx_hash.is_empty());
        assert!(prepared.transaction.get("inputs").is_some());

        let reqs = server.received_requests().await.unwrap();
        assert!(reqs.iter().all(|r| r.url.path() != "/v1/transactions"));
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

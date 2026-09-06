//! HTTP client for the Lineage /v1 API.

use std::collections::BTreeMap;

use serde::de::DeserializeOwned;

use crate::error::{ApiProblem, Error, Result};
use crate::models::{BalancesResponse, DebugData, PaymentAccepted, Supply, TxStatus};

#[derive(Debug, Clone)]
pub struct Hosts {
    pub mempool: String,
    pub storage: String,
    pub miner: String,
}

#[derive(Debug, Clone)]
pub struct Client {
    http: reqwest::Client,
    hosts: Hosts,
    api_key: Option<String>,
}

impl Client {
    pub fn new(hosts: Hosts) -> Result<Self> {
        Ok(Client {
            http: reqwest::Client::builder().build()?,
            hosts,
            api_key: None,
        })
    }

    pub fn with_api_key(mut self, key: impl Into<String>) -> Self {
        self.api_key = Some(key.into());
        self
    }

    pub fn testnet() -> Result<Client> {
        Client::new(Hosts {
            mempool: "https://mempool.lineage.to".into(),
            storage: "https://storage.lineage.to".into(),
            miner: "https://miner.lineage.to".into(),
        })
    }

    pub(crate) async fn get_json<T: DeserializeOwned>(
        &self,
        base: &str,
        path: &str,
        query: &[(&str, String)],
    ) -> Result<T> {
        let url = format!("{base}{path}");
        let mut req = self.http.get(url).query(query);
        if let Some(key) = &self.api_key {
            req = req.header("x-api-key", key);
        }
        let resp = req.send().await?;
        let status = resp.status();
        let bytes = resp.bytes().await?;
        if status.is_success() {
            Ok(serde_json::from_slice(&bytes)?)
        } else {
            let problem: ApiProblem = serde_json::from_slice(&bytes).unwrap_or(ApiProblem {
                status: status.as_u16(),
                title: None,
                detail: Some(String::from_utf8_lossy(&bytes).into_owned()),
                request_id: None,
            });
            Err(Error::Api(problem))
        }
    }

    pub(crate) async fn post_json<T: DeserializeOwned>(
        &self,
        base: &str,
        path: &str,
        body: &serde_json::Value,
    ) -> Result<T> {
        let url = format!("{base}{path}");
        let mut req = self.http.post(url).json(body);
        if let Some(key) = &self.api_key {
            req = req.header("x-api-key", key);
        }
        let resp = req.send().await?;
        let status = resp.status();
        let bytes = resp.bytes().await?;
        if status.is_success() {
            Ok(serde_json::from_slice(&bytes)?)
        } else {
            let problem: ApiProblem = serde_json::from_slice(&bytes).unwrap_or(ApiProblem {
                status: status.as_u16(),
                title: None,
                detail: Some(String::from_utf8_lossy(&bytes).into_owned()),
                request_id: None,
            });
            Err(Error::Api(problem))
        }
    }
    async fn send_empty(
        &self,
        method: reqwest::Method,
        base: &str,
        path: &str,
        body: &serde_json::Value,
    ) -> Result<()> {
        let url = format!("{base}{path}");
        let mut req = self.http.request(method, url).json(body);
        if let Some(key) = &self.api_key {
            req = req.header("x-api-key", key);
        }
        let resp = req.send().await?;
        let status = resp.status();
        if status.is_success() {
            Ok(())
        } else {
            let bytes = resp.bytes().await?;
            let problem: ApiProblem = serde_json::from_slice(&bytes).unwrap_or(ApiProblem {
                status: status.as_u16(),
                title: None,
                detail: Some(String::from_utf8_lossy(&bytes).into_owned()),
                request_id: None,
            });
            Err(Error::Api(problem))
        }
    }

    pub(crate) async fn put_empty(&self, base: &str, path: &str, body: &serde_json::Value) -> Result<()> {
        self.send_empty(reqwest::Method::PUT, base, path, body).await
    }

    pub(crate) async fn post_empty(&self, base: &str, path: &str, body: &serde_json::Value) -> Result<()> {
        self.send_empty(reqwest::Method::POST, base, path, body).await
    }
}

#[derive(Debug, Clone, Copy)]
pub enum NodeClass {
    Mempool,
    Storage,
    Miner,
}

impl Client {
    fn base_for(&self, class: NodeClass) -> String {
        match class {
            NodeClass::Mempool => self.hosts.mempool.clone(),
            NodeClass::Storage => self.hosts.storage.clone(),
            NodeClass::Miner => self.hosts.miner.clone(),
        }
    }

    pub async fn supply(&self) -> Result<Supply> {
        let base = self.base_for(NodeClass::Mempool);
        self.get_json(&base, "/v1/supply", &[]).await
    }

    pub async fn balances(&self, addresses: &[&str]) -> Result<BalancesResponse> {
        let base = self.base_for(NodeClass::Mempool);
        let query: Vec<(&str, String)> = addresses.iter().map(|a| ("address", a.to_string())).collect();
        self.get_json(&base, "/v1/balances", &query).await
    }

    pub async fn transaction_status(&self, tx_hash: &str) -> Result<BTreeMap<String, TxStatus>> {
        let base = self.base_for(NodeClass::Mempool);
        self.get_json(&base, "/v1/transactions/status", &[("tx_hash", tx_hash.to_string())]).await
    }

    pub async fn latest_block(&self) -> Result<serde_json::Value> {
        let base = self.base_for(NodeClass::Storage);
        self.get_json(&base, "/v1/blocks/latest", &[]).await
    }

    pub async fn block_by_num(&self, num: u64) -> Result<serde_json::Value> {
        let base = self.base_for(NodeClass::Storage);
        self.get_json(&base, &format!("/v1/blocks/{num}"), &[]).await
    }

    pub async fn blocks(&self, nums: &[u64]) -> Result<serde_json::Value> {
        let base = self.base_for(NodeClass::Storage);
        let query: Vec<(&str, String)> = nums.iter().map(|n| ("num", n.to_string())).collect();
        self.get_json(&base, "/v1/blocks", &query).await
    }

    pub async fn blockchain_entry(&self, key: &str) -> Result<serde_json::Value> {
        let base = self.base_for(NodeClass::Storage);
        self.get_json(&base, &format!("/v1/blockchain-entries/{key}"), &[]).await
    }

    pub async fn current_mining_block(&self) -> Result<serde_json::Value> {
        let base = self.base_for(NodeClass::Miner);
        self.get_json(&base, "/v1/mining/current-block", &[]).await
    }

    pub async fn debug(&self, class: NodeClass) -> Result<DebugData> {
        let base = self.base_for(class);
        self.get_json(&base, "/v1/debug", &[]).await
    }

    /// Submits a raw item (e.g. a transaction or block) to the mempool's item store.
    pub async fn post_items(&self, body: serde_json::Value) -> Result<serde_json::Value> {
        let base = self.base_for(NodeClass::Mempool);
        self.post_json(&base, "/v1/items", &body).await
    }

    /// Queries balances for a batch of addresses via the mempool's POST endpoint.
    pub async fn query_balances(&self, addresses: &[&str]) -> Result<BalancesResponse> {
        let base = self.base_for(NodeClass::Mempool);
        let body = serde_json::json!({ "addresses": addresses });
        self.post_json(&base, "/v1/balances/query", &body).await
    }

    /// Queries transaction status for a batch of hashes via the mempool's POST endpoint.
    pub async fn query_transaction_status(&self, hashes: &[&str]) -> Result<BTreeMap<String, TxStatus>> {
        let base = self.base_for(NodeClass::Mempool);
        let body = serde_json::json!({ "hashes": hashes });
        self.post_json(&base, "/v1/transactions/status:query", &body).await
    }

    /// Serializes transactions to their wire hex form via the coupled user node.
    pub async fn serialize_transactions(&self, txs: serde_json::Value) -> Result<serde_json::Value> {
        let base = self.base_for(NodeClass::Miner);
        self.post_json(&base, "/v1/transactions:serialize", &txs).await
    }

    /// Deserializes wire hex transactions back to their JSON form via the coupled user node.
    pub async fn deserialize_transactions(&self, hexes: serde_json::Value) -> Result<serde_json::Value> {
        let base = self.base_for(NodeClass::Miner);
        self.post_json(&base, "/v1/transactions:deserialize", &hexes).await
    }

    /// Fetches wallet status (running total, etc.) from the given node.
    pub async fn wallet_info(&self, base: NodeClass) -> Result<serde_json::Value> {
        let base = self.base_for(base);
        self.get_json(&base, "/v1/wallet", &[]).await
    }

    /// Generates a new wallet address on the given node.
    pub async fn new_wallet_address(&self, base: NodeClass) -> Result<serde_json::Value> {
        let base = self.base_for(base);
        self.post_json(&base, "/v1/wallet/addresses", &serde_json::json!({})).await
    }

    /// Fetches the given node's stored keypairs.
    pub async fn get_keypairs(&self, base: NodeClass) -> Result<serde_json::Value> {
        let base = self.base_for(base);
        self.get_json(&base, "/v1/wallet/keypairs", &[]).await
    }

    /// Imports keypairs into the given node's wallet.
    pub async fn import_keypairs(&self, base: NodeClass, body: serde_json::Value) -> Result<serde_json::Value> {
        let base = self.base_for(base);
        self.post_json(&base, "/v1/wallet/keypairs", &body).await
    }

    /// Changes the given node's wallet passphrase.
    pub async fn change_passphrase(&self, base: NodeClass, old: &str, new: &str) -> Result<()> {
        let base = self.base_for(base);
        let body = serde_json::json!({ "old_passphrase": old, "new_passphrase": new });
        self.put_empty(&base, "/v1/wallet/passphrase", &body).await
    }

    /// Refreshes the given node's wallet running total.
    pub async fn refresh_running_total(&self, base: NodeClass, body: serde_json::Value) -> Result<serde_json::Value> {
        let base = self.base_for(base);
        self.post_json(&base, "/v1/wallet/running-total:refresh", &body).await
    }

    /// Fetches the given node's outgoing (pending) transactions.
    pub async fn outgoing_transactions(&self, base: NodeClass) -> Result<serde_json::Value> {
        let base = self.base_for(base);
        self.get_json(&base, "/v1/transactions/outgoing", &[]).await
    }

    /// Requests a testnet donation to the given address from the miner.
    pub async fn request_donation(&self, address: &str) -> Result<()> {
        let base = self.base_for(NodeClass::Miner);
        let body = serde_json::json!({ "address": address });
        self.post_empty(&base, "/v1/donation-requests", &body).await
    }

    /// Submits signed transactions to the mempool for inclusion.
    pub async fn submit_transactions(&self, txs: &[serde_json::Value]) -> Result<serde_json::Value> {
        let base = self.base_for(NodeClass::Mempool);
        let body = serde_json::json!({ "transactions": txs });
        self.post_json(&base, "/v1/transactions", &body).await
    }

    /// Requests a node-signed payment from the miner's wallet.
    pub async fn make_payment(
        &self,
        kind: &str,
        address: &str,
        amount: u64,
        passphrase: &str,
        locktime: Option<u64>,
    ) -> Result<PaymentAccepted> {
        let base = self.base_for(NodeClass::Miner);
        let body = serde_json::json!({
            "kind": kind,
            "address": address,
            "amount": amount,
            "passphrase": passphrase,
            "locktime": locktime,
        });
        self.post_json(&base, "/v1/payments", &body).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::matchers::{header, method, path};
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
    async fn get_json_decodes_success_body() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/v1/ping"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"ok": true})))
            .mount(&server)
            .await;
        let client = client_for(&server);
        let v: serde_json::Value = client.get_json(&client.hosts.mempool.clone(), "/v1/ping", &[]).await.unwrap();
        assert_eq!(v["ok"], true);
    }

    #[tokio::test]
    async fn get_json_maps_problem_to_api_error() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/v1/blocks/9999"))
            .respond_with(
                ResponseTemplate::new(404)
                    .insert_header("content-type", "application/problem+json")
                    .set_body_json(serde_json::json!({"status":404,"detail":"No block at that height"})),
            )
            .mount(&server)
            .await;
        let client = client_for(&server);
        let err = client
            .get_json::<serde_json::Value>(&client.hosts.storage.clone(), "/v1/blocks/9999", &[])
            .await
            .unwrap_err();
        match err {
            Error::Api(p) => {
                assert_eq!(p.status, 404);
                assert_eq!(p.detail.as_deref(), Some("No block at that height"));
            }
            other => panic!("expected Api error, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn get_json_sends_api_key_header() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/v1/debug"))
            .and(header("x-api-key", "secret"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({})))
            .mount(&server)
            .await;
        let client = client_for(&server).with_api_key("secret");
        let _: serde_json::Value = client.get_json(&client.hosts.mempool.clone(), "/v1/debug", &[]).await.unwrap();
        // If the header did not match, wiremock returns 404 and this unwrap panics.
    }

    #[tokio::test]
    async fn supply_hits_mempool_and_types_response() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/v1/supply"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"total":360360000000000000u64,"issued":42})))
            .mount(&server)
            .await;
        let client = client_for(&server);
        let s = client.supply().await.unwrap();
        assert_eq!(s.issued, 42);
    }

    #[tokio::test]
    async fn balances_sends_repeated_address_query() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/v1/balances"))
            .and(wiremock::matchers::query_param("address", "a1"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "balance": {"address_list": {}, "total": {"tokens": 0, "items": {}}}
            })))
            .mount(&server)
            .await;
        let client = client_for(&server);
        let r = client.balances(&["a1"]).await.unwrap();
        assert_eq!(r.balance.total.tokens, 0);
    }

    #[tokio::test]
    async fn submit_transactions_posts_envelope() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/transactions"))
            .respond_with(ResponseTemplate::new(201).set_body_json(serde_json::json!({"transactions": {}})))
            .mount(&server)
            .await;
        let client = client_for(&server);
        let r = client
            .submit_transactions(&[serde_json::json!({"inputs":[],"outputs":[],"version":1,"fees":null,"druid_info":null})])
            .await
            .unwrap();
        assert!(r["transactions"].is_object());
    }

    #[tokio::test]
    async fn post_items_posts_to_mempool() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/items"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"accepted": true})))
            .mount(&server)
            .await;
        let client = client_for(&server);
        let r = client.post_items(serde_json::json!({"key": "value"})).await.unwrap();
        assert_eq!(r["accepted"], true);
    }

    #[tokio::test]
    async fn query_balances_posts_addresses() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/balances/query"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "balance": {"address_list": {}, "total": {"tokens": 5, "items": {}}}
            })))
            .mount(&server)
            .await;
        let client = client_for(&server);
        let r = client.query_balances(&["a1", "a2"]).await.unwrap();
        assert_eq!(r.balance.total.tokens, 5);
    }

    #[tokio::test]
    async fn query_transaction_status_posts_hashes() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/transactions/status:query"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "h1": {"status": "Confirmed", "timestamp": 123, "additional_info": ""}
            })))
            .mount(&server)
            .await;
        let client = client_for(&server);
        let r = client.query_transaction_status(&["h1"]).await.unwrap();
        assert_eq!(r["h1"].status, "Confirmed");
    }

    #[tokio::test]
    async fn serialize_transactions_hits_miner() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/transactions:serialize"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!(["deadbeef"])))
            .mount(&server)
            .await;
        let client = client_for(&server);
        let r = client
            .serialize_transactions(serde_json::json!({"transactions": []}))
            .await
            .unwrap();
        assert_eq!(r[0], "deadbeef");
    }

    #[tokio::test]
    async fn deserialize_transactions_hits_miner() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/transactions:deserialize"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"transactions": []})))
            .mount(&server)
            .await;
        let client = client_for(&server);
        let r = client
            .deserialize_transactions(serde_json::json!(["deadbeef"]))
            .await
            .unwrap();
        assert!(r["transactions"].is_array());
    }

    #[tokio::test]
    async fn wallet_info_gets_wallet() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/v1/wallet"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"running_total": 0})))
            .mount(&server)
            .await;
        let client = client_for(&server);
        let r = client.wallet_info(NodeClass::Miner).await.unwrap();
        assert_eq!(r["running_total"], 0);
    }

    #[tokio::test]
    async fn new_wallet_address_posts_empty_body() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/wallet/addresses"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"address": "addr1"})))
            .mount(&server)
            .await;
        let client = client_for(&server);
        let r = client.new_wallet_address(NodeClass::Miner).await.unwrap();
        assert_eq!(r["address"], "addr1");
    }

    #[tokio::test]
    async fn get_keypairs_gets_wallet_keypairs() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/v1/wallet/keypairs"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"keypairs": []})))
            .mount(&server)
            .await;
        let client = client_for(&server);
        let r = client.get_keypairs(NodeClass::Miner).await.unwrap();
        assert!(r["keypairs"].is_array());
    }

    #[tokio::test]
    async fn import_keypairs_posts_wallet_keypairs() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/wallet/keypairs"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"imported": 1})))
            .mount(&server)
            .await;
        let client = client_for(&server);
        let r = client
            .import_keypairs(NodeClass::Miner, serde_json::json!({"keypairs": []}))
            .await
            .unwrap();
        assert_eq!(r["imported"], 1);
    }

    #[tokio::test]
    async fn change_passphrase_puts_and_returns_unit_on_204() {
        let server = MockServer::start().await;
        Mock::given(method("PUT"))
            .and(path("/v1/wallet/passphrase"))
            .respond_with(ResponseTemplate::new(204))
            .mount(&server)
            .await;
        let client = client_for(&server);
        client.change_passphrase(NodeClass::Miner, "old", "new").await.unwrap();
    }

    #[tokio::test]
    async fn refresh_running_total_posts_body() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/wallet/running-total:refresh"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"running_total": 42})))
            .mount(&server)
            .await;
        let client = client_for(&server);
        let r = client
            .refresh_running_total(NodeClass::Miner, serde_json::json!({"addresses": []}))
            .await
            .unwrap();
        assert_eq!(r["running_total"], 42);
    }

    #[tokio::test]
    async fn outgoing_transactions_gets_path() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/v1/transactions/outgoing"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"transactions": []})))
            .mount(&server)
            .await;
        let client = client_for(&server);
        let r = client.outgoing_transactions(NodeClass::Miner).await.unwrap();
        assert!(r["transactions"].is_array());
    }

    #[tokio::test]
    async fn request_donation_posts_and_returns_unit_on_202() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/donation-requests"))
            .respond_with(ResponseTemplate::new(202))
            .mount(&server)
            .await;
        let client = client_for(&server);
        client.request_donation("addr1").await.unwrap();
    }

    #[tokio::test]
    async fn latest_block_hits_storage() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/v1/blocks/latest"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"block":{"block":{"header":{"b_num":5373}}}})))
            .mount(&server)
            .await;
        let client = client_for(&server);
        let v = client.latest_block().await.unwrap();
        assert_eq!(v["block"]["block"]["header"]["b_num"], 5373);
    }
}

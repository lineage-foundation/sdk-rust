//! HTTP client for the Lineage /v1 API.

use std::collections::BTreeMap;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use serde::de::DeserializeOwned;
use tw_chain::primitives::asset::Asset;

use crate::druid::FetchBalanceResponse;
use crate::error::{ApiProblem, Error, Result};
use crate::models::{BalancesResponse, DebugData, ItemInfo, PaymentAccepted, Supply, TxStatus};

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
    /// Per-instance cache of resolved item genesis facts, keyed by
    /// `genesis_hash`. `Arc<Mutex<..>>` so cheap `Client` clones share one
    /// cache (like sdk-js's per-`Wallet` `Map`); no TTL, since genesis facts
    /// are immutable. Only successful (HTTP 200) resolves are stored, so a
    /// transient storage error stays retryable. The guard is only ever held
    /// for a synchronous read/insert, never across an `.await`.
    item_info_cache: Arc<Mutex<HashMap<String, ItemInfo>>>,
}

impl Client {
    pub fn new(hosts: Hosts) -> Result<Self> {
        Ok(Client {
            http: reqwest::Client::builder().build()?,
            hosts,
            api_key: None,
            item_info_cache: Arc::new(Mutex::new(HashMap::new())),
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

    /// Fetches balances for the given addresses with `address_list` decoded
    /// in its original JSON key order (see [`FetchBalanceResponse`]) --
    /// load-bearing for two-way trade input selection, which must walk
    /// addresses in the same order sdk-go/sdk-js/sdk-php do.
    pub async fn balances_ordered(&self, addresses: &[&str]) -> Result<FetchBalanceResponse> {
        #[derive(serde::Deserialize)]
        struct Envelope {
            balance: FetchBalanceResponse,
        }

        let base = self.base_for(NodeClass::Mempool);
        let query: Vec<(&str, String)> = addresses.iter().map(|a| ("address", a.to_string())).collect();
        let envelope: Envelope = self.get_json(&base, "/v1/balances", &query).await?;
        Ok(envelope.balance)
    }

    /// Fetches balances for `addresses` with `address_list` in original JSON
    /// key order (as [`Self::balances_ordered`]) and, when `enrich` is true,
    /// attaches each item's genesis `metadata` resolved from the storage node.
    ///
    /// Enrichment is best-effort: the DISTINCT item `genesis_hash`es are
    /// resolved CONCURRENTLY against the storage node's
    /// `GET /v1/items/{genesis_hash}`, cached per client instance, and written
    /// onto each item's `metadata`. A resolver error, 404, or missing metadata
    /// leaves that item's existing `metadata` untouched -- the balance call
    /// itself never fails because of enrichment. Pass `enrich = false` to skip
    /// enrichment entirely (zero resolver calls).
    pub async fn fetch_balance(&self, addresses: &[&str], enrich: bool) -> Result<FetchBalanceResponse> {
        let mut balance = self.balances_ordered(addresses).await?;
        if enrich {
            self.enrich_balance_items(&mut balance).await;
        }
        Ok(balance)
    }

    /// Attaches genesis `metadata` to every item UTXO in a balance (best-effort).
    /// Resolves the distinct cache-miss `genesis_hash`es concurrently, then
    /// writes each successfully resolved `metadata` onto its item. Items whose
    /// hash did not resolve are left exactly as they were (no clobber).
    async fn enrich_balance_items(&self, balance: &mut FetchBalanceResponse) {
        // Collect the distinct item genesis hashes in first-seen order.
        let mut seen = std::collections::HashSet::new();
        let mut distinct: Vec<String> = Vec::new();
        for utxos in balance.address_list.values() {
            for utxo in utxos {
                if let Asset::Item(item) = &utxo.value {
                    if let Some(hash) = &item.genesis_hash {
                        if seen.insert(hash.clone()) {
                            distinct.push(hash.clone());
                        }
                    }
                }
            }
        }
        if distinct.is_empty() {
            return;
        }

        // Only resolve hashes not already cached (dedup across repeat listings).
        let misses: Vec<String> = {
            let cache = self.item_info_cache.lock().expect("item info cache mutex poisoned");
            distinct.into_iter().filter(|hash| !cache.contains_key(hash)).collect()
        };
        if !misses.is_empty() {
            let resolves = misses.iter().map(|hash| self.resolve_item_info_best_effort(hash));
            futures::future::join_all(resolves).await;
        }

        // Write back: only overwrite metadata for hashes that resolved (are cached).
        let cache = self.item_info_cache.lock().expect("item info cache mutex poisoned");
        for utxos in balance.address_list.values_mut() {
            for utxo in utxos {
                if let Asset::Item(item) = &mut utxo.value {
                    if let Some(hash) = &item.genesis_hash {
                        if let Some(info) = cache.get(hash) {
                            item.metadata = info.metadata.clone();
                        }
                    }
                }
            }
        }
    }

    /// Resolves an item's genesis facts (metadata, supply, provenance) from the
    /// storage node by its `genesis_hash`, using the per-instance cache. A 200
    /// resolve is cached permanently (genesis facts are immutable); a 404 or
    /// other non-200 is returned as an error and NOT cached, so it stays
    /// retryable.
    pub async fn get_item_info(&self, genesis_hash: &str) -> Result<ItemInfo> {
        if let Some(cached) = self.cached_item_info(genesis_hash) {
            return Ok(cached);
        }
        let info = self.request_item_info(genesis_hash).await?;
        self.store_item_info(genesis_hash, info.clone());
        Ok(info)
    }

    /// One raw storage GET for an item's genesis facts, without touching the
    /// cache. 404/other non-200 surface as `Err(Error::Api)`.
    async fn request_item_info(&self, genesis_hash: &str) -> Result<ItemInfo> {
        let base = self.base_for(NodeClass::Storage);
        self.get_json(&base, &format!("/v1/items/{genesis_hash}"), &[]).await
    }

    /// Cache read (clones out so the lock is not held across an `.await`).
    fn cached_item_info(&self, genesis_hash: &str) -> Option<ItemInfo> {
        self.item_info_cache
            .lock()
            .expect("item info cache mutex poisoned")
            .get(genesis_hash)
            .cloned()
    }

    /// Cache write for a successful resolve.
    fn store_item_info(&self, genesis_hash: &str, info: ItemInfo) {
        self.item_info_cache
            .lock()
            .expect("item info cache mutex poisoned")
            .insert(genesis_hash.to_string(), info);
    }

    /// Best-effort resolve used by balance enrichment: resolves one item and
    /// caches it on success, swallowing any error so enrichment can never fail
    /// the listing.
    async fn resolve_item_info_best_effort(&self, genesis_hash: &str) {
        if let Ok(info) = self.request_item_info(genesis_hash).await {
            self.store_item_info(genesis_hash, info);
        }
    }

    /// The base URL of this client's configured mempool host.
    pub fn mempool_host(&self) -> &str {
        &self.hosts.mempool
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
        self.submit_transactions_to(&base, txs).await
    }

    /// Submits signed transactions to an arbitrary mempool host's
    /// `/v1/transactions`, rather than this client's own configured mempool.
    /// Two-way trades name the mempool host to submit to as part of the
    /// [`crate::models::Pending2WTxDetails`] both parties exchange over
    /// valence, since it need not be this client's own.
    pub async fn submit_transactions_to(&self, host: &str, txs: &[serde_json::Value]) -> Result<serde_json::Value> {
        let body = serde_json::json!({ "transactions": txs });
        self.post_json(host, "/v1/transactions", &body).await
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

    const GH: &str = "genesis0abc";

    fn item_info_body() -> serde_json::Value {
        serde_json::json!({
            "genesis_hash": GH,
            "metadata": "ticket #1",
            "total_amount": 1000,
            "created": { "block_num": 42, "tx_hash": GH },
            "creator_address": "addr_creator"
        })
    }

    fn item_path() -> String {
        format!("/v1/items/{GH}")
    }

    #[tokio::test]
    async fn get_item_info_returns_genesis_facts_on_200() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path(item_path()))
            .respond_with(ResponseTemplate::new(200).set_body_json(item_info_body()))
            .mount(&server)
            .await;
        let client = client_for(&server);
        let info = client.get_item_info(GH).await.unwrap();
        assert_eq!(info.genesis_hash, GH);
        assert_eq!(info.metadata.as_deref(), Some("ticket #1"));
        assert_eq!(info.total_amount, 1000);
        assert_eq!(info.created.block_num, 42);
        assert_eq!(info.creator_address.as_deref(), Some("addr_creator"));
    }

    #[tokio::test]
    async fn get_item_info_caches_second_call_issues_no_http() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path(item_path()))
            .respond_with(ResponseTemplate::new(200).set_body_json(item_info_body()))
            .mount(&server)
            .await;
        let client = client_for(&server);
        let first = client.get_item_info(GH).await.unwrap();
        let second = client.get_item_info(GH).await.unwrap();
        assert_eq!(first, second);
        let hits = server
            .received_requests()
            .await
            .unwrap()
            .into_iter()
            .filter(|r| r.url.path() == item_path())
            .count();
        assert_eq!(hits, 1, "second call must be served from cache");
    }

    #[tokio::test]
    async fn get_item_info_404_errors_and_is_not_cached() {
        let server = MockServer::start().await;
        // First request 404s (exhausts after one match), then a later request 200s.
        Mock::given(method("GET"))
            .and(path(item_path()))
            .respond_with(ResponseTemplate::new(404))
            .up_to_n_times(1)
            .with_priority(1)
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path(item_path()))
            .respond_with(ResponseTemplate::new(200).set_body_json(item_info_body()))
            .with_priority(2)
            .mount(&server)
            .await;
        let client = client_for(&server);
        assert!(client.get_item_info(GH).await.is_err(), "404 must be an error");
        // Not cached -> the retry actually hits the network and succeeds.
        let retried = client.get_item_info(GH).await.unwrap();
        assert_eq!(retried.metadata.as_deref(), Some("ticket #1"));
        let hits = server
            .received_requests()
            .await
            .unwrap()
            .into_iter()
            .filter(|r| r.url.path() == item_path())
            .count();
        assert_eq!(hits, 2, "a failed resolve must stay retryable");
    }

    fn balance_envelope(address_list: serde_json::Value) -> serde_json::Value {
        serde_json::json!({
            "balance": {
                "address_list": address_list,
                "total": { "tokens": 0, "items": { GH: 5 } }
            }
        })
    }

    fn item_utxo(t_hash: &str, genesis_hash: &str, metadata: serde_json::Value) -> serde_json::Value {
        serde_json::json!({
            "out_point": { "n": 0, "t_hash": t_hash },
            "value": { "Item": { "amount": 5, "genesis_hash": genesis_hash, "metadata": metadata } }
        })
    }

    fn item_metadata(balance: &crate::druid::FetchBalanceResponse, address: &str, idx: usize) -> Option<String> {
        match &balance.address_list[address][idx].value {
            tw_chain::primitives::asset::Asset::Item(item) => item.metadata.clone(),
            other => panic!("expected an item asset, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn fetch_balance_enriches_item_metadata_by_default() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/v1/balances"))
            .respond_with(ResponseTemplate::new(200).set_body_json(balance_envelope(serde_json::json!({
                "addr1": [ item_utxo("t0", GH, serde_json::Value::Null) ]
            }))))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path(item_path()))
            .respond_with(ResponseTemplate::new(200).set_body_json(item_info_body()))
            .mount(&server)
            .await;
        let client = client_for(&server);
        let balance = client.fetch_balance(&["addr1"], true).await.unwrap();
        assert_eq!(item_metadata(&balance, "addr1", 0).as_deref(), Some("ticket #1"));
    }

    #[tokio::test]
    async fn fetch_balance_dedups_and_caches_one_resolver_call_per_hash() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/v1/balances"))
            .respond_with(ResponseTemplate::new(200).set_body_json(balance_envelope(serde_json::json!({
                "addr1": [ item_utxo("t0", GH, serde_json::Value::Null) ],
                "addr2": [ item_utxo("t1", GH, serde_json::Value::Null) ]
            }))))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path(item_path()))
            .respond_with(ResponseTemplate::new(200).set_body_json(item_info_body()))
            .mount(&server)
            .await;
        let client = client_for(&server);

        let first = client.fetch_balance(&["addr1", "addr2"], true).await.unwrap();
        assert_eq!(item_metadata(&first, "addr1", 0).as_deref(), Some("ticket #1"));
        assert_eq!(item_metadata(&first, "addr2", 0).as_deref(), Some("ticket #1"));

        // Repeat listing must issue no further resolver calls (cache hit).
        let second = client.fetch_balance(&["addr1", "addr2"], true).await.unwrap();
        assert_eq!(item_metadata(&second, "addr1", 0).as_deref(), Some("ticket #1"));

        let resolver_hits = server
            .received_requests()
            .await
            .unwrap()
            .into_iter()
            .filter(|r| r.url.path() == item_path())
            .count();
        assert_eq!(resolver_hits, 1, "one distinct hash across two addrs and two listings => one call");
    }

    #[tokio::test]
    async fn fetch_balance_graceful_degrade_on_resolver_error() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/v1/balances"))
            .respond_with(ResponseTemplate::new(200).set_body_json(balance_envelope(serde_json::json!({
                "addr1": [ item_utxo("t0", GH, serde_json::Value::Null) ]
            }))))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path(item_path()))
            .respond_with(ResponseTemplate::new(500))
            .mount(&server)
            .await;
        let client = client_for(&server);
        let balance = client.fetch_balance(&["addr1"], true).await.unwrap();
        assert!(item_metadata(&balance, "addr1", 0).is_none(), "resolver error => metadata stays null, call still Ok");
    }

    #[tokio::test]
    async fn fetch_balance_resolve_miss_does_not_clobber_inline_metadata() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/v1/balances"))
            .respond_with(ResponseTemplate::new(200).set_body_json(balance_envelope(serde_json::json!({
                "addr1": [ item_utxo("t0", GH, serde_json::json!("inline meta")) ]
            }))))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path(item_path()))
            .respond_with(ResponseTemplate::new(500))
            .mount(&server)
            .await;
        let client = client_for(&server);
        let balance = client.fetch_balance(&["addr1"], true).await.unwrap();
        assert_eq!(
            item_metadata(&balance, "addr1", 0).as_deref(),
            Some("inline meta"),
            "a failed resolve must not overwrite metadata the item already carried"
        );
    }

    #[tokio::test]
    async fn fetch_balance_opt_out_issues_no_resolver_calls() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/v1/balances"))
            .respond_with(ResponseTemplate::new(200).set_body_json(balance_envelope(serde_json::json!({
                "addr1": [ item_utxo("t0", GH, serde_json::Value::Null) ]
            }))))
            .mount(&server)
            .await;
        // Register a resolver mock so we can assert it is NEVER consumed.
        Mock::given(method("GET"))
            .and(path(item_path()))
            .respond_with(ResponseTemplate::new(200).set_body_json(item_info_body()))
            .mount(&server)
            .await;
        let client = client_for(&server);
        let balance = client.fetch_balance(&["addr1"], false).await.unwrap();
        assert!(item_metadata(&balance, "addr1", 0).is_none(), "enrich=false leaves items unmodified");
        let resolver_hits = server
            .received_requests()
            .await
            .unwrap()
            .into_iter()
            .filter(|r| r.url.path() == item_path())
            .count();
        assert_eq!(resolver_hits, 0, "enrich=false must issue zero resolver calls");
    }
}

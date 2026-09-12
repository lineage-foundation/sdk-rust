//! End-to-end two-way (DRUID) payment flow: the valence mailbox client and
//! the four flow methods (`make_2way_payment`, `fetch_pending_2way_payment`,
//! `accept_2way_payment`, `reject_2way_payment`), exercised against a
//! wiremock double of both a valence host and the mempool `/v1/*` API.
//!
//! Test (a) is verified against the shared cross-SDK vector in
//! `tests/fixtures/twoway.json` (also used by sdk-go, sdk-php, and sdk-js);
//! the rest exercise wallet-driven roundtrips against fresh wallet keys.

use std::collections::HashMap;

use lineage_sdk::{
    Client, CreateTransaction, DruidExpectation, DruidInfo, Hosts, LocalSigner, Pending2WTxDetails,
    Pending2WTxStatus, PendingHalf, ValenceClient, Wallet,
};
use tw_chain::crypto::sign_ed25519 as sign;
use tw_chain::primitives::asset::{Asset, TokenAmount};
use tw_chain::primitives::transaction::TxOut;
use wiremock::matchers::{body_partial_json, header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn fixture() -> serde_json::Value {
    let raw = include_str!("fixtures/twoway.json");
    serde_json::from_str(raw).expect("valid fixture JSON")
}

/// Rebuilds a ring-compatible PKCS8 v2 Ed25519 document from the raw
/// 32-byte seed + 32-byte public key format the shared fixture uses for
/// secret keys. See `tests/twoway.rs` for the identical helper.
fn secret_key_from_fixture(secret_key_hex: &str) -> sign::SecretKey {
    let raw = hex::decode(secret_key_hex).expect("valid hex secret key");
    let (seed, public_key) = raw.split_at(32);

    let mut pkcs8 = Vec::with_capacity(85);
    pkcs8.extend_from_slice(&[
        0x30, 0x53, 0x02, 0x01, 0x01, 0x30, 0x05, 0x06, 0x03, 0x2b, 0x65, 0x70, 0x04, 0x22, 0x04,
        0x20,
    ]);
    pkcs8.extend_from_slice(seed);
    pkcs8.extend_from_slice(&[0xa1, 0x23, 0x03, 0x21, 0x00]);
    pkcs8.extend_from_slice(public_key);

    sign::SecretKey::from_slice(&pkcs8).expect("well-formed PKCS8 document")
}

fn public_key_from_hex(hex_str: &str) -> sign::PublicKey {
    let raw = hex::decode(hex_str).expect("valid hex public key");
    sign::PublicKey::from_slice(&raw).expect("valid public key bytes")
}

fn client_for(server: &MockServer) -> Client {
    Client::new(Hosts {
        mempool: server.uri(),
        storage: server.uri(),
        miner: server.uri(),
    })
    .unwrap()
}

fn dummy_create_transaction(druid: &str, to: &str, amount: u64) -> CreateTransaction {
    CreateTransaction {
        inputs: vec![],
        outputs: vec![TxOut {
            value: Asset::Token(TokenAmount(amount)),
            locktime: 0,
            script_public_key: Some(to.to_string()),
        }],
        druid_info: DruidInfo {
            druid: druid.to_string(),
            participants: 2,
            expectations: vec![DruidExpectation {
                from: String::new(),
                to: to.to_string(),
                asset: Asset::Token(TokenAmount(amount)),
            }],
            genesis_hash: None,
        },
    }
}

// (a) valence POST auth headers match the twoway.json valenceAuth vector,
// and the offer body is posted in plaintext.
#[tokio::test]
async fn valence_post_signs_with_raw_address_and_sends_plaintext_offer() {
    let fixture = fixture();
    let auth = &fixture["valenceAuth"];
    let ours = &fixture["fixedKeypairs"]["ours"][0];

    let address = auth["address"].as_str().unwrap().to_string();
    assert_eq!(ours["address"].as_str().unwrap(), address);

    let public_key = public_key_from_hex(ours["public_key"].as_str().unwrap());
    let secret_key = secret_key_from_fixture(ours["secret_key"].as_str().unwrap());

    let details: Pending2WTxDetails =
        serde_json::from_value(fixture["pending2WTxDetailsOffer"].clone()).expect("valid offer");

    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/messages"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({})))
        .mount(&server)
        .await;

    let valence = ValenceClient::new(server.uri()).unwrap();
    valence
        .post(&address, &public_key, &secret_key, &details)
        .await
        .unwrap();

    let requests = server.received_requests().await.unwrap();
    assert_eq!(requests.len(), 1);
    let req = &requests[0];

    assert_eq!(
        req.headers.get("address").unwrap().to_str().unwrap(),
        auth["address"].as_str().unwrap()
    );
    assert_eq!(
        req.headers.get("public_key").unwrap().to_str().unwrap(),
        auth["public_key"].as_str().unwrap()
    );
    assert_eq!(
        req.headers.get("signature").unwrap().to_str().unwrap(),
        auth["signature"].as_str().unwrap()
    );

    let body: serde_json::Value = serde_json::from_slice(&req.body).unwrap();
    assert_eq!(body["id"], fixture["pending2WTxDetailsOffer"]["druid"]);
    assert_eq!(body["data"]["status"], "pending");
    assert_eq!(
        body["data"]["senderExpectation"],
        fixture["pending2WTxDetailsOffer"]["senderExpectation"]
    );
    assert_eq!(
        body["data"]["receiverExpectation"],
        fixture["pending2WTxDetailsOffer"]["receiverExpectation"]
    );
}

// (b) ACCEPTOR discovery: no stored halves, an offer sitting in this
// wallet's own mailbox is surfaced in `pending`.
#[tokio::test]
async fn fetch_pending_discovers_incoming_offer_with_no_stored_halves() {
    let dir = tempfile::tempdir().unwrap();
    let mut wallet = Wallet::create(&dir.path().join("w.json"), "pw").unwrap();
    let my_address = wallet.new_address().unwrap();

    let server = MockServer::start().await;
    let druid = "DRUID0xincoming00000000000000000000".to_string();
    let offer = Pending2WTxDetails {
        druid: druid.clone(),
        sender_expectation: DruidExpectation {
            from: "counterparty-inputs".into(),
            to: my_address.clone(),
            asset: Asset::Token(TokenAmount(10)),
        },
        receiver_expectation: DruidExpectation {
            from: my_address.clone(),
            to: "counterparty-address".into(),
            asset: Asset::Token(TokenAmount(20)),
        },
        status: Pending2WTxStatus::Pending,
        mempool_host: server.uri(),
    };

    Mock::given(method("GET"))
        .and(path("/messages"))
        .and(header("address", my_address.as_str()))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(HashMap::from([(druid.clone(), &offer)])),
        )
        .mount(&server)
        .await;

    let client = client_for(&server);
    let signer = LocalSigner::new(&client, &wallet, "change").with_valence_host(server.uri());

    let (pending, settled, error) = signer.fetch_pending_2way_payment(&[], &[my_address]).await;

    assert!(error.is_none(), "unexpected error: {error:?}");
    assert!(settled.is_empty());
    assert_eq!(pending.get(&druid), Some(&offer));
}

// (c) INITIATOR settle: an accepted stored half is decrypted, submitted,
// deleted from valence, and reported settled.
#[tokio::test]
async fn fetch_pending_settles_accepted_stored_half() {
    let dir = tempfile::tempdir().unwrap();
    let mut wallet = Wallet::create(&dir.path().join("w.json"), "pw").unwrap();
    let my_address = wallet.new_address().unwrap();

    let druid = "DRUID0xsettleme000000000000000000000".to_string();
    let tx = dummy_create_transaction(&druid, "counterparty-address", 500);
    let encrypted_half = wallet.encrypt_transaction(&tx).unwrap();

    let stored = PendingHalf {
        druid: druid.clone(),
        encrypted_half,
        sender_expectation: tx.druid_info.expectations[0].clone(),
        receiver_expectation: DruidExpectation {
            from: "counterparty-address".into(),
            to: my_address.clone(),
            asset: Asset::Token(TokenAmount(500)),
        },
    };

    let server = MockServer::start().await;
    let filled_sender_expectation = DruidExpectation {
        from: "counterparty-filled-inputs".into(),
        to: "counterparty-address".into(),
        asset: Asset::Token(TokenAmount(500)),
    };
    let accepted = Pending2WTxDetails {
        druid: druid.clone(),
        sender_expectation: filled_sender_expectation.clone(),
        receiver_expectation: stored.receiver_expectation.clone(),
        status: Pending2WTxStatus::Accepted,
        mempool_host: server.uri(),
    };

    Mock::given(method("GET"))
        .and(path("/messages"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(HashMap::from([(druid.clone(), accepted)])),
        )
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/v1/transactions"))
        .respond_with(
            ResponseTemplate::new(201).set_body_json(serde_json::json!({"transactions": {}})),
        )
        .mount(&server)
        .await;
    Mock::given(method("DELETE"))
        .and(path(format!("/messages/{druid}")))
        .respond_with(ResponseTemplate::new(200))
        .mount(&server)
        .await;

    let client = client_for(&server);
    let signer = LocalSigner::new(&client, &wallet, "change").with_valence_host(server.uri());

    let (pending, settled, error) = signer
        .fetch_pending_2way_payment(&[stored], &[my_address])
        .await;

    assert!(error.is_none(), "unexpected error: {error:?}");
    assert_eq!(settled, vec![druid.clone()]);
    assert!(!pending.contains_key(&druid));

    let submit_reqs: Vec<_> = server
        .received_requests()
        .await
        .unwrap()
        .into_iter()
        .filter(|r| r.url.path() == "/v1/transactions")
        .collect();
    assert_eq!(submit_reqs.len(), 1);
    let body: serde_json::Value = serde_json::from_slice(&submit_reqs[0].body).unwrap();
    assert_eq!(body["transactions"][0]["version"], 2);
    assert_eq!(body["transactions"][0]["fees"], serde_json::Value::Null);
    assert_eq!(
        body["transactions"][0]["druid_info"]["genesis_hash"],
        serde_json::Value::Null
    );
    assert_eq!(
        body["transactions"][0]["druid_info"]["expectations"][0]["from"],
        "counterparty-filled-inputs"
    );
}

// (d) accept submits to details.mempool_host with druid_info.genesis_hash
// null and fees null, then posts the accepted status to valence.
#[tokio::test]
async fn accept_submits_to_details_mempool_host_with_null_fees_and_genesis_hash() {
    let dir = tempfile::tempdir().unwrap();
    let mut wallet = Wallet::create(&dir.path().join("w.json"), "pw").unwrap();
    let acceptor_address = wallet.new_address().unwrap();

    let client_server = MockServer::start().await;
    let remote_mempool = MockServer::start().await;

    let mut address_list = serde_json::Map::new();
    address_list.insert(
        acceptor_address.clone(),
        serde_json::json!([{"out_point": {"n": 0, "t_hash": "g0000"}, "value": {"Token": 1000}}]),
    );
    Mock::given(method("GET"))
        .and(path("/v1/balances"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "balance": {"address_list": address_list, "total": {"tokens": 1000, "items": {}}}
        })))
        .mount(&client_server)
        .await;
    Mock::given(method("POST"))
        .and(path("/messages"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({})))
        .mount(&client_server)
        .await;
    Mock::given(method("POST"))
        .and(path("/v1/transactions"))
        .respond_with(
            ResponseTemplate::new(201).set_body_json(serde_json::json!({"transactions": {}})),
        )
        .mount(&remote_mempool)
        .await;

    let details = Pending2WTxDetails {
        druid: "DRUID0xaccepted0000000000000000000000".into(),
        sender_expectation: DruidExpectation {
            from: String::new(),
            to: "offerer-address".into(),
            asset: Asset::Token(TokenAmount(500)),
        },
        receiver_expectation: DruidExpectation {
            from: String::new(),
            to: acceptor_address.clone(),
            asset: Asset::Token(TokenAmount(300)),
        },
        status: Pending2WTxStatus::Pending,
        mempool_host: remote_mempool.uri(),
    };

    let client = client_for(&client_server);
    let signer =
        LocalSigner::new(&client, &wallet, "change").with_valence_host(client_server.uri());

    signer
        .accept_2way_payment(details, &[acceptor_address])
        .await
        .unwrap();

    let submit_reqs = remote_mempool.received_requests().await.unwrap();
    assert_eq!(submit_reqs.len(), 1);
    let body: serde_json::Value = serde_json::from_slice(&submit_reqs[0].body).unwrap();
    assert_eq!(body["transactions"][0]["version"], 2);
    assert_eq!(body["transactions"][0]["fees"], serde_json::Value::Null);
    assert_eq!(
        body["transactions"][0]["druid_info"]["genesis_hash"],
        serde_json::Value::Null
    );

    // Nothing must ever hit the wallet's own configured mempool host.
    let own_mempool_hits: Vec<_> = client_server
        .received_requests()
        .await
        .unwrap()
        .into_iter()
        .filter(|r| r.url.path() == "/v1/transactions")
        .collect();
    assert!(own_mempool_hits.is_empty());

    let valence_reqs: Vec<_> = client_server
        .received_requests()
        .await
        .unwrap()
        .into_iter()
        .filter(|r| r.url.path() == "/messages")
        .collect();
    assert_eq!(valence_reqs.len(), 1);
    let posted: serde_json::Value = serde_json::from_slice(&valence_reqs[0].body).unwrap();
    assert_eq!(posted["data"]["status"], "accepted");
    assert_ne!(posted["data"]["senderExpectation"]["from"], "");
}

// (e) reject posts the rejected status with no transaction submission.
#[tokio::test]
async fn reject_posts_rejected_status_without_submitting() {
    let dir = tempfile::tempdir().unwrap();
    let mut wallet = Wallet::create(&dir.path().join("w.json"), "pw").unwrap();
    let acceptor_address = wallet.new_address().unwrap();

    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/messages"))
        .and(body_partial_json(
            serde_json::json!({"data": {"status": "rejected"}}),
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({})))
        .mount(&server)
        .await;

    let details = Pending2WTxDetails {
        druid: "DRUID0xrejected0000000000000000000000".into(),
        sender_expectation: DruidExpectation {
            from: String::new(),
            to: "offerer-address".into(),
            asset: Asset::Token(TokenAmount(500)),
        },
        receiver_expectation: DruidExpectation {
            from: String::new(),
            to: acceptor_address.clone(),
            asset: Asset::Token(TokenAmount(300)),
        },
        status: Pending2WTxStatus::Pending,
        mempool_host: server.uri(),
    };

    let client = client_for(&server);
    let signer = LocalSigner::new(&client, &wallet, "change").with_valence_host(server.uri());

    signer
        .reject_2way_payment(details, &[acceptor_address])
        .await
        .unwrap();

    let submit_reqs: Vec<_> = server
        .received_requests()
        .await
        .unwrap()
        .into_iter()
        .filter(|r| r.url.path() == "/v1/transactions")
        .collect();
    assert!(submit_reqs.is_empty());
}

// (f) A mid-loop mailbox fetch error must not discard pending/settled work
// already accumulated against the other mailboxes.
#[tokio::test]
async fn fetch_pending_returns_partial_results_alongside_error() {
    let dir = tempfile::tempdir().unwrap();
    let mut wallet = Wallet::create(&dir.path().join("w.json"), "pw").unwrap();
    let bad_address = wallet.new_address().unwrap();
    let good_address = wallet.new_address().unwrap();

    let server = MockServer::start().await;
    let druid = "DRUID0xgoodmailbox00000000000000000".to_string();
    let offer = Pending2WTxDetails {
        druid: druid.clone(),
        sender_expectation: DruidExpectation {
            from: "x".into(),
            to: good_address.clone(),
            asset: Asset::Token(TokenAmount(1)),
        },
        receiver_expectation: DruidExpectation {
            from: good_address.clone(),
            to: "y".into(),
            asset: Asset::Token(TokenAmount(2)),
        },
        status: Pending2WTxStatus::Pending,
        mempool_host: server.uri(),
    };

    Mock::given(method("GET"))
        .and(path("/messages"))
        .and(header("address", bad_address.as_str()))
        .respond_with(ResponseTemplate::new(500))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/messages"))
        .and(header("address", good_address.as_str()))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(HashMap::from([(druid.clone(), offer.clone())])),
        )
        .mount(&server)
        .await;

    let client = client_for(&server);
    let signer = LocalSigner::new(&client, &wallet, "change").with_valence_host(server.uri());

    let (pending, settled, error) = signer
        .fetch_pending_2way_payment(&[], &[bad_address, good_address])
        .await;

    assert!(
        error.is_some(),
        "expected the bad mailbox's failure to surface"
    );
    assert!(settled.is_empty());
    assert_eq!(pending.get(&druid), Some(&offer));
}

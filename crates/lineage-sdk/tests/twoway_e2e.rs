//! Live two-wallet, two-way (DRUID) atomic swap against the shared Lineage
//! testnet deployment: wallet A mints an item and offers it in exchange for
//! tokens from wallet B; B accepts; A settles. Both wallets' final balances
//! are polled to confirm the swap landed atomically (A ends up with the
//! tokens, B ends up with the item).
//!
//! This funds fresh wallets from the testnet miner faucet and submits real
//! transactions, so it never runs under plain `cargo test` -- it's
//! `#[ignore]`d, and even under `--ignored` it skips itself unless
//! `LINEAGE_E2E=1` is set. Mirrors sdk-go's `TestTwoWaySwap_Live`
//! (`twoway_e2e_test.go`).
//!
//! Run with:
//!
//!     LINEAGE_E2E=1 cargo test --test twoway_e2e -- --ignored --nocapture

use std::time::Duration;

use lineage_sdk::{Client, FetchBalanceResponse, Hosts, LocalSigner, Wallet};
use tw_chain::primitives::asset::{Asset, ItemAsset, TokenAmount};

const E2E_MEMPOOL_HOST: &str = "https://mempool.lineage.to";
const E2E_STORAGE_HOST: &str = "https://storage.lineage.to";
const E2E_VALENCE_HOST: &str = "https://valence.lineage.to";
const E2E_MINER_HOST: &str = "https://miner.lineage.to";
const E2E_DEFAULT_GENESIS_HASH: &str = "default_genesis_hash";

const ITEM_AMOUNT: u64 = 50; // items A mints and offers
const TOKEN_AMOUNT: u64 = 100; // tokens B pays and A receives
const FUND_TOKENS_A: u64 = 1000; // just enough for A to exist as a funded address
const FUND_TOKENS_B: u64 = 1000; // must cover TOKEN_AMOUNT

fn e2e_client() -> Client {
    Client::new(Hosts {
        mempool: E2E_MEMPOOL_HOST.to_string(),
        storage: E2E_STORAGE_HOST.to_string(),
        miner: E2E_MINER_HOST.to_string(),
    })
    .expect("client construction cannot fail")
}

/// Polls `address`'s balance every 5s, up to 24 tries (~2 minutes), until
/// `want` reports satisfied against the resulting [`FetchBalanceResponse`].
async fn poll_balance(
    client: &Client,
    address: &str,
    want: impl Fn(&FetchBalanceResponse) -> bool,
) -> FetchBalanceResponse {
    let mut last: Option<FetchBalanceResponse> = None;
    for _ in 0..24 {
        let balance = client
            .balances_ordered(&[address])
            .await
            .expect("balances_ordered should succeed against the live testnet");
        if want(&balance) {
            return balance;
        }
        last = Some(balance);
        tokio::time::sleep(Duration::from_secs(5)).await;
    }
    panic!(
        "poll timeout for {address}: last balance total={:?}",
        last.map(|b| b.total)
    );
}

// TestTwoWaySwap_Live's Rust counterpart: drives a full two-wallet, two-way
// (DRUID) atomic swap against the live Lineage testnet.
#[tokio::test]
#[ignore = "hits the live testnet; run with `LINEAGE_E2E=1 cargo test --test twoway_e2e -- --ignored --nocapture`"]
async fn two_wallet_swap_settles_atomically() {
    if std::env::var("LINEAGE_E2E").as_deref() != Ok("1") {
        eprintln!("skipping: set LINEAGE_E2E=1 to run the live two-wallet 2-way swap");
        return;
    }

    let client = e2e_client();

    let dir = tempfile::tempdir().expect("create temp dir for wallet keystores");

    eprintln!("== wallet A: create, fund, mint item ==");
    let mut wallet_a = Wallet::create(&dir.path().join("wallet-a.json"), "e2e-wallet-a-pass")
        .expect("create wallet A");
    let address_a = wallet_a.new_address().expect("A: new address");

    client
        .make_payment("address", &address_a, FUND_TOKENS_A, "", None)
        .await
        .expect("A: fund from miner");
    poll_balance(&client, &address_a, |b| b.total.tokens > 0).await;

    let signer_a = LocalSigner::new(&client, &wallet_a, address_a.clone())
        .with_valence_host(E2E_VALENCE_HOST.to_string());
    signer_a
        .create_items(&address_a, true, ITEM_AMOUNT, None)
        .await
        .expect("A: create items");
    poll_balance(&client, &address_a, |b| {
        b.total
            .items
            .get(E2E_DEFAULT_GENESIS_HASH)
            .copied()
            .unwrap_or(0)
            >= ITEM_AMOUNT
    })
    .await;

    eprintln!("== wallet B: create, fund with tokens ==");
    let mut wallet_b = Wallet::create(&dir.path().join("wallet-b.json"), "e2e-wallet-b-pass")
        .expect("create wallet B");
    let address_b = wallet_b.new_address().expect("B: new address");

    client
        .make_payment("address", &address_b, FUND_TOKENS_B, "", None)
        .await
        .expect("B: fund from miner");
    poll_balance(&client, &address_b, |b| b.total.tokens >= TOKEN_AMOUNT).await;

    let signer_b = LocalSigner::new(&client, &wallet_b, address_b.clone())
        .with_valence_host(E2E_VALENCE_HOST.to_string());

    eprintln!("== A: make_2way_payment (offer item, want tokens) ==");
    let sending_asset = Asset::Item(ItemAsset {
        amount: ITEM_AMOUNT,
        genesis_hash: Some(E2E_DEFAULT_GENESIS_HASH.to_string()),
        metadata: None,
    });
    let receiving_asset = Asset::Token(TokenAmount(TOKEN_AMOUNT));
    let half = signer_a
        .make_2way_payment(
            &address_b,
            sending_asset,
            receiving_asset,
            std::slice::from_ref(&address_a),
            &address_a,
        )
        .await
        .expect("A: make_2way_payment");

    eprintln!("== B: fetch_pending_2way_payment sees the offer, then accept_2way_payment ==");
    let (pending, _settled, error) = signer_b
        .fetch_pending_2way_payment(&[], std::slice::from_ref(&address_b))
        .await;
    assert!(
        error.is_none(),
        "B: fetch_pending_2way_payment error: {error:?}"
    );
    let offer = pending.get(&half.druid).cloned().unwrap_or_else(|| {
        panic!(
            "B did not see A's offer for druid {} (pending={pending:?})",
            half.druid
        )
    });

    signer_b
        .accept_2way_payment(offer, std::slice::from_ref(&address_b))
        .await
        .expect("B: accept_2way_payment");

    eprintln!("== A: fetch_pending_2way_payment second pass settles the swap ==");
    let (pending, settled, error) = signer_a
        .fetch_pending_2way_payment(
            std::slice::from_ref(&half),
            std::slice::from_ref(&address_a),
        )
        .await;
    assert!(
        error.is_none(),
        "A: fetch_pending_2way_payment error: {error:?}"
    );
    assert!(
        settled.contains(&half.druid),
        "druid {} was not settled (settled={settled:?}, pending={pending:?})",
        half.druid
    );

    eprintln!("== poll final balances: A should hold the tokens, B should hold the item ==");
    let final_a = poll_balance(&client, &address_a, |b| b.total.tokens >= TOKEN_AMOUNT).await;
    let final_b = poll_balance(&client, &address_b, |b| {
        b.total
            .items
            .get(E2E_DEFAULT_GENESIS_HASH)
            .copied()
            .unwrap_or(0)
            >= ITEM_AMOUNT
    })
    .await;

    eprintln!(
        "FINAL A tokens={} items={:?}",
        final_a.total.tokens, final_a.total.items
    );
    eprintln!(
        "FINAL B tokens={} items={:?}",
        final_b.total.tokens, final_b.total.items
    );

    assert!(
        final_a.total.tokens >= TOKEN_AMOUNT,
        "A: expected >= {TOKEN_AMOUNT} tokens after the swap, got {}",
        final_a.total.tokens
    );
    assert!(
        final_b
            .total
            .items
            .get(E2E_DEFAULT_GENESIS_HASH)
            .copied()
            .unwrap_or(0)
            >= ITEM_AMOUNT,
        "B: expected >= {ITEM_AMOUNT} items after the swap, got {:?}",
        final_b.total.items.get(E2E_DEFAULT_GENESIS_HASH)
    );
}

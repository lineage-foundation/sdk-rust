# sdk-rust
Rust SDK for Lineage Foundation

## Two-way (DRUID) payments

A two-way payment is an atomic swap between two parties, brokered through
[Valence](https://github.com/lineage-foundation/valence), a plaintext message-relay
service used only to exchange trade offers/acceptances -- it never sees keys or signs
anything. Configure the valence host on a `LocalSigner` alongside its `Client`/`Wallet`:

```rust
let client = Client::new(Hosts {
    mempool: "https://mempool.lineage.to".into(),
    storage: "https://storage.lineage.to".into(),
    miner: "https://miner.lineage.to".into(),
})?;
let signer = LocalSigner::new(&client, &wallet, change_address)
    .with_valence_host("https://valence.lineage.to");
```

Two-way payments are unusable until `with_valence_host` is set.

The flow is four `LocalSigner` methods:

```rust
// Party A offers sending_asset to payment_address in exchange for receiving_asset,
// paid to receive_keypair's address. all_keypairs sources the inputs for A's own
// half. Returns a PendingHalf -- persist it; fetch_pending_2way_payment needs it
// later to recognize and settle the trade once accepted.
pub async fn make_2way_payment(
    &self,
    payment_address: &str,
    sending_asset: Asset,
    receiving_asset: Asset,
    all_keypairs: &[String],
    receive_keypair: &str,
) -> Result<PendingHalf>;

// Party B (or A, on a later poll) checks its mailboxes for offers matching
// stored: any that the counterparty has accepted are settled (submitted to
// this wallet's own mempool) and returned in settled; everything else still
// outstanding is returned in pending, keyed by DRUID. A failure against one
// mailbox never discards progress made against the others.
pub async fn fetch_pending_2way_payment(
    &self,
    stored: &[PendingHalf],
    all_keypairs: &[String],
) -> (HashMap<String, Pending2WTxDetails>, Vec<String>, Option<Error>);

// Party B accepts a pending offer: pays details.sender_expectation's asset,
// submits the transaction to details.mempool_host, and posts the acceptance
// back to valence so A's next fetch_pending_2way_payment settles it.
pub async fn accept_2way_payment(
    &self,
    details: Pending2WTxDetails,
    all_keypairs: &[String],
) -> Result<()>;

// Party B declines instead: no transaction is built, only the rejected
// status is posted back to valence.
pub async fn reject_2way_payment(
    &self,
    details: Pending2WTxDetails,
    all_keypairs: &[String],
) -> Result<()>;
```

A full round trip -- A offers an item for tokens, B accepts, A settles:

```rust
let sending_asset = Asset::Item(ItemAsset {
    amount: 50,
    genesis_hash: Some("default_genesis_hash".into()),
    metadata: None,
});
let receiving_asset = Asset::Token(TokenAmount(100));

let half = signer_a
    .make_2way_payment(&b_address, sending_asset, receiving_asset, &[a_address.clone()], &a_address)
    .await?;
// persist half (keyed by half.druid) until it settles

// ...on B's side, out of band:
let (pending, _, error) = signer_b.fetch_pending_2way_payment(&[], &[b_address.clone()]).await;
let offer = pending[&half.druid].clone();
signer_b.accept_2way_payment(offer, &[b_address.clone()]).await?;

// ...back on A's side, a later poll settles it:
let (_, settled, error) = signer_a
    .fetch_pending_2way_payment(&[half.clone()], &[a_address.clone()])
    .await;
// settled now contains half.druid; A holds the tokens, B holds the item.
```

`make_2way_payment` builds and seals this party's transaction half under the wallet's
own keystore master key before it's ever handed back to the caller: `PendingHalf` is
safe to persist as-is (e.g. to disk, or wherever the caller keeps outstanding trades)
between the offer going out and `fetch_pending_2way_payment` reporting it settled. No
per-call state is otherwise kept on `LocalSigner` or `Client` -- every method call is
fully self-contained given a stored `PendingHalf` or a `Pending2WTxDetails` handed back
from valence.

This is wire- and protocol-compatible with sdk-js's, sdk-go's, and sdk-php's two-way
payment support: any of the four SDKs can make the offer, accept it, or settle it, in
any combination -- they all speak the same DRUID transaction shape and the same
plaintext Valence mailbox format.

See `crates/lineage-sdk/tests/twoway_flow.rs` for wiremock-backed coverage of all four
methods, and `crates/lineage-sdk/tests/twoway_e2e.rs` for a complete two-wallet live
example.

## Two-way payment live e2e

`crates/lineage-sdk/tests/twoway_e2e.rs` drives a complete two-wallet atomic swap
against the live Lineage testnet: it creates wallets A and B, funds both from the
testnet miner's faucet, mints an item to A, has A offer that item to B in exchange for
tokens, has B accept, has A settle, and polls both wallets' balances to confirm the
swap landed atomically (A ends up with the tokens, B ends up with the item).

Because it funds real wallets and submits live transactions, it's `#[ignore]`d --
`cargo test` never runs it -- and even under `--ignored` it skips itself unless
`LINEAGE_E2E=1` is set:

```bash
LINEAGE_E2E=1 cargo test --test twoway_e2e -- --ignored --nocapture
```

## Testing

```bash
cargo build
cargo test                                            # unit + integration tests (fast, no network)
LINEAGE_E2E=1 cargo test --test twoway_e2e -- --ignored --nocapture  # + two-way live e2e
cargo clippy --all-targets -- -D warnings
```

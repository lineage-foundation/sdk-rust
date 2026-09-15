# Lineage Rust SDK

Rust SDK for the Lineage `/v1` REST API: a keyless read client and a key-holding wallet that signs transactions locally.

## Installation

Published on [crates.io](https://crates.io/crates/lineage-sdk) as `lineage-sdk`.

```bash
cargo add lineage-sdk
```

## Configuration

```rust
let client = Client::new(Hosts {
    mempool: "https://mempool.lineage.to".into(),
    storage: "https://storage.lineage.to".into(),
    miner: "https://miner.lineage.to".into(),
})?
.with_api_key("optional-key"); // only if your node requires x-api-key
```

## Quickstart

```rust
use std::path::Path;
use lineage_sdk::{Client, Hosts, LocalSigner, Signer, Wallet};

#[tokio::main]
async fn main() -> lineage_sdk::Result<()> {
    let client = Client::new(Hosts {
        mempool: "https://mempool.lineage.to".into(),
        storage: "https://storage.lineage.to".into(),
        miner: "https://miner.lineage.to".into(),
    })?;

    let mut wallet = Wallet::create(Path::new("wallet.json"), "passphrase")?;
    let address = wallet.new_address()?;

    let balances = client.balances(&[&address]).await?;
    println!("balance: {:?}", balances.balance.total);

    let signer = LocalSigner::new(&client, &wallet, address.clone());
    let receipt = signer.pay("recipient-address", 1_000).await?;
    println!("paid {} tokens, tx {}", receipt.amount, receipt.tx_hash);

    Ok(())
}
```

## Two-way (DRUID) payments

A two-way payment is an atomic swap between two parties, brokered through
[Valence](https://github.com/lineage-foundation/valence), a plaintext message-relay
service used only to exchange trade offers/acceptances -- it never sees keys or signs
anything. Configure the valence host on a `LocalSigner` alongside its `Client`/`Wallet`:

```rust
let signer = LocalSigner::new(&client, &wallet, change_address)
    .with_valence_host("https://valence.lineage.to");
```

Two-way payments are unusable until `with_valence_host` is set. The flow is four
`LocalSigner` methods:

```rust
// Party A offers sending_asset to payment_address in exchange for receiving_asset,
// paid to receive_keypair's address. Returns a PendingHalf -- persist it;
// fetch_pending_2way_payment needs it later to recognize and settle the trade.
signer_a.make_2way_payment(payment_address, sending_asset, receiving_asset, all_keypairs, receive_keypair).await?;

// Party B (or A, on a later poll) checks its mailboxes: offers the counterparty
// has accepted are settled and returned in `settled`; the rest, keyed by DRUID,
// come back in `pending`.
signer_b.fetch_pending_2way_payment(stored, all_keypairs).await;

// Party B accepts: pays sender_expectation's asset, submits the transaction, and
// posts the acceptance back to valence so A's next fetch settles it.
signer_b.accept_2way_payment(details, all_keypairs).await?;

// Party B declines instead: no transaction is built, only the rejected status
// is posted back to valence.
signer_b.reject_2way_payment(details, all_keypairs).await?;
```

A full round trip -- A offers an item for tokens, B accepts, A settles:

```rust
let sending_asset = Asset::Item(ItemAsset { amount: 50, genesis_hash: Some("default_genesis_hash".into()), metadata: None });
let receiving_asset = Asset::Token(TokenAmount(100));

let half = signer_a
    .make_2way_payment(&b_address, sending_asset, receiving_asset, &[a_address.clone()], &a_address)
    .await?;
// persist half (keyed by half.druid) until it settles

// ...on B's side, out of band:
let (pending, _, _) = signer_b.fetch_pending_2way_payment(&[], &[b_address.clone()]).await;
let offer = pending[&half.druid].clone();
signer_b.accept_2way_payment(offer, &[b_address.clone()]).await?;

// ...back on A's side, a later poll settles it:
let (_, settled, _) = signer_a.fetch_pending_2way_payment(&[half.clone()], &[a_address.clone()]).await;
// settled now contains half.druid; A holds the tokens, B holds the item.
```

See `crates/lineage-sdk/tests/twoway_flow.rs` for wiremock-backed coverage of all four
methods, and `crates/lineage-sdk/tests/twoway_e2e.rs` for a complete two-wallet live
example against the testnet.

## Wire compatibility

Keys and signatures are byte-for-byte compatible across every Lineage SDK -- a wallet (mnemonic) created in one derives the same addresses and produces the same signatures in all of them. sdk-js is the reference implementation; BIP39/BIP32 derivation, SHA3-256 addresses, ed25519 signing, and the `/v1` transaction serialization (field order is load-bearing -- you sign exactly what you submit) all match it exactly.

Two-way trades interoperate across all the SDKs and settle atomically through the mempool's DRUID pool, so either party can be on any SDK.

## Testing

```bash
cargo test                                                            # unit + integration tests, no network
LINEAGE_E2E=1 cargo test --test twoway_e2e -- --ignored --nocapture   # + two-way live e2e against testnet
```

## Lineage SDKs

- [JavaScript / TypeScript](https://github.com/lineage-foundation/sdk-js)
- [Python](https://github.com/lineage-foundation/sdk-python)
- [Go](https://github.com/lineage-foundation/sdk-go)
- [Rust](https://github.com/lineage-foundation/sdk-rust)
- [PHP](https://github.com/lineage-foundation/sdk-php)
- [Laravel](https://github.com/lineage-foundation/sdk-laravel)

## License

MIT — see [LICENSE](LICENSE).

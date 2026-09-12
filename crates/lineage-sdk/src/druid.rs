//! Two-way (DDE) trade construction: DRUID generation, the P2PKH-inputs
//! correlation address, and building one half of a two-way trade.
//!
//! A DDE half is an ordinary P2PKH transaction: `druid_info` is UNSIGNED and
//! is never folded into any signable preimage. Each input is signed exactly
//! as in the one-way path (see [`crate::tx::build_signed_payment`]), over
//! the same signable hash: `sha3_256(concat(json(outputs)) + json(previous_out))`,
//! with the resulting hex string signed as-is.

use std::collections::HashMap;

use indexmap::IndexMap;
use serde::{Deserialize, Serialize};

use tw_chain::crypto::sha3_256;
use tw_chain::crypto::sign_ed25519 as sign;
use tw_chain::primitives::asset::{Asset, ItemAsset, TokenAmount};
use tw_chain::primitives::transaction::{OutPoint, TxIn, TxOut};
use tw_chain::script::lang::Script;
use tw_chain::utils::transaction_utils::{
    construct_address, construct_tx_in_out_signable_hash,
    construct_tx_ins_address as tw_construct_tx_ins_address,
};

use crate::error::{Error, Result};
use crate::models::{DruidExpectation, DruidInfo};

/// Signing material keyed by address, as passed to [`create_2w_tx_half`].
pub type KeyPairs = HashMap<String, (sign::PublicKey, sign::SecretKey)>;

/// One UTXO entry as returned in a balance response's `address_list`.
#[derive(Debug, Clone, Deserialize)]
pub struct BalanceUtxo {
    pub out_point: OutPoint,
    pub value: Asset,
}

/// Aggregate totals from a balance response, used for an up-front
/// sufficient-funds check.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct BalanceTotals {
    #[serde(default)]
    pub tokens: u64,
    #[serde(default)]
    pub items: HashMap<String, u64>,
}

/// A balance response decoded so that `address_list` retains its original
/// JSON key order.
///
/// Input selection walks addresses in that order to match sdk-js's
/// `Object.entries(...)` (and sdk-go/sdk-php, which replicate it)
/// byte-for-byte. A `BTreeMap` (alphabetical) or a `HashMap` (unspecified)
/// would select a different -- and incompatible -- set of inputs whenever
/// a balance's addresses aren't already in alphabetical order.
#[derive(Debug, Clone, Deserialize)]
pub struct FetchBalanceResponse {
    pub address_list: IndexMap<String, Vec<BalanceUtxo>>,
    #[serde(default)]
    pub total: BalanceTotals,
}

/// The `script_signature` of a [`CreateTxIn`], JSON-tagged to match the
/// node's `/v1/transactions` construction DTO
/// (`fleet-api::v1::tx_convert::CreateTxInScript`).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum CreateTxInScript {
    Pay2PkH {
        signable_data: String,
        signature: String,
        public_key: String,
        address_version: Option<u64>,
    },
}

/// One input of a [`CreateTransaction`].
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CreateTxIn {
    pub previous_out: OutPoint,
    pub script_signature: CreateTxInScript,
}

/// The transaction format version sent in `/v1/transactions` bodies (matches
/// sdk-js/sdk-go/sdk-php). Added at submission time (see
/// [`CreateTransaction::to_submission_value`]); not part of the constructed
/// half or the signable preimage.
const TRANSACTION_VERSION: u64 = 2;

/// A constructed, per-input-signed transaction ready for submission to
/// `/v1/transactions`, carrying unsigned DDE trade metadata.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CreateTransaction {
    pub inputs: Vec<CreateTxIn>,
    pub outputs: Vec<TxOut>,
    pub druid_info: DruidInfo,
}

impl CreateTransaction {
    /// The `/v1/transactions` submission wire shape for this half: an
    /// explicit `fees: null` and `druid_info.genesis_hash: null` are added.
    /// Neither is part of the transaction's signed preimage -- `fees` isn't
    /// present on [`CreateTransaction`] at all, and `genesis_hash` is
    /// intentionally omitted by [`create_2w_tx_half`] -- but the node
    /// requires both present, explicitly null, at submission time. Matches
    /// sdk-go's `druidInfoForSubmission` / `createTxSubmission` and sdk-js's
    /// `{ ...tx, fees: null }` / `{ ...tx.druid_info, genesis_hash: null }`
    /// spreads.
    pub fn to_submission_value(&self) -> Result<serde_json::Value> {
        let mut value = serde_json::to_value(self)?;
        // The transaction format version. Not part of the signable preimage and
        // absent from the constructed half (sdk-js `create2WTxHalf` omits it),
        // but the node requires it present at submission time. Matches the
        // `version: 2` that sdk-js/sdk-go/sdk-php send for `/v1/transactions`.
        value["version"] = serde_json::Value::from(TRANSACTION_VERSION);
        value["fees"] = serde_json::Value::Null;
        if let Some(druid_info) = value.get_mut("druid_info") {
            druid_info["genesis_hash"] = serde_json::Value::Null;
        }
        Ok(value)
    }
}

/// Generates a fresh DRUID (DDE receipt unique identifier) used to correlate
/// the two halves of a two-way trade: `"DRUID0x"` followed by the first 32
/// hex characters of `hex(sha3_256(uuid))`, where `uuid` is a random UUIDv4
/// with its dashes stripped.
///
/// Matches sdk-js/sdk-go/sdk-php byte-for-byte: the DRUID is a hash of the
/// UUID's hex text, not of its raw 16 bytes.
pub fn generate_druid() -> String {
    let uuid_hex = uuid::Uuid::new_v4().simple().to_string();
    let digest = sha3_256::digest(uuid_hex.as_bytes());
    let hash_hex = hex::encode(digest);
    format!("DRUID0x{}", &hash_hex[..32])
}

fn asset_amount(asset: &Asset) -> u64 {
    match asset {
        Asset::Token(TokenAmount(amount)) => *amount,
        Asset::Item(item) => item.amount,
    }
}

fn assets_compatible(a: &Asset, b: &Asset) -> bool {
    match (a, b) {
        (Asset::Token(_), Asset::Token(_)) => true,
        (Asset::Item(x), Asset::Item(y)) => x.genesis_hash == y.genesis_hash,
        _ => false,
    }
}

fn with_amount(asset: &Asset, amount: u64) -> Asset {
    match asset {
        Asset::Token(_) => Asset::Token(TokenAmount(amount)),
        Asset::Item(item) => Asset::Item(ItemAsset {
            amount,
            genesis_hash: item.genesis_hash.clone(),
            metadata: item.metadata.clone(),
        }),
    }
}

fn has_enough_funds(asset: &Asset, totals: &BalanceTotals) -> bool {
    match asset {
        Asset::Token(TokenAmount(amount)) => *amount <= totals.tokens,
        Asset::Item(item) => {
            let available = item
                .genesis_hash
                .as_deref()
                .and_then(|hash| totals.items.get(hash))
                .copied()
                .unwrap_or(0);
            item.amount <= available
        }
    }
}

/// Builds one half of a two-way (DRUID) trade: an ordinary P2PKH
/// transaction that pays `counter_expectation.asset` to
/// `counter_expectation.to` (plus any excess back to `excess_address`),
/// carrying `this_expectation` as this party's half of the DRUID trade
/// metadata.
///
/// Input selection and per-input signing are exactly the one-way path's:
/// inputs are gathered by walking `balance.address_list` in its original
/// JSON order, and every input is signed over
/// `construct_tx_in_out_signable_hash(previous_out, outputs)` -- the same
/// signable hash [`crate::tx::build_signed_payment`] uses. `druid_info` is
/// attached unsigned and is never part of that preimage.
pub fn create_2w_tx_half(
    druid: &str,
    this_expectation: DruidExpectation,
    counter_expectation: DruidExpectation,
    balance: &FetchBalanceResponse,
    keypairs: &KeyPairs,
    excess_address: &str,
    locktime: u64,
) -> Result<CreateTransaction> {
    let payment_asset = &counter_expectation.asset;

    if !has_enough_funds(payment_asset, &balance.total) {
        return Err(Error::Tx("insufficient funds".into()));
    }

    let payment_amount = asset_amount(payment_asset);
    let mut total_amount: u64 = 0;
    let mut drafts: Vec<(OutPoint, sign::PublicKey, sign::SecretKey, Option<u64>)> = Vec::new();

    for (address, utxos) in &balance.address_list {
        let (public_key, secret_key) = keypairs
            .get(address)
            .ok_or_else(|| Error::Tx(format!("no keypair for address {address}")))?;

        let address_version = if construct_address(public_key) == *address {
            None
        } else {
            return Err(Error::Tx(format!(
                "address {address} does not match the default derivation for its public key"
            )));
        };

        for utxo in utxos {
            if total_amount >= payment_amount {
                break;
            }
            if !assets_compatible(payment_asset, &utxo.value) {
                continue;
            }
            drafts.push((
                utxo.out_point.clone(),
                *public_key,
                secret_key.clone(),
                address_version,
            ));
            total_amount += asset_amount(&utxo.value);
        }
    }

    if drafts.is_empty() {
        return Err(Error::Tx("no inputs available to cover payment".into()));
    }

    let mut outputs = vec![TxOut {
        value: payment_asset.clone(),
        locktime,
        script_public_key: Some(counter_expectation.to.clone()),
    }];

    if total_amount > payment_amount {
        outputs.push(TxOut {
            value: with_amount(payment_asset, total_amount - payment_amount),
            locktime: 0,
            script_public_key: Some(excess_address.to_string()),
        });
    }

    let mut inputs = Vec::with_capacity(drafts.len());
    for (previous_out, public_key, secret_key, address_version) in drafts {
        let signable_in = TxIn {
            previous_out: Some(previous_out.clone()),
            script_signature: Script::new(),
        };
        let signable_data = construct_tx_in_out_signable_hash(&signable_in, &outputs);
        let signature = sign::sign_detached(signable_data.as_bytes(), &secret_key);

        inputs.push(CreateTxIn {
            previous_out,
            script_signature: CreateTxInScript::Pay2PkH {
                signable_data,
                signature: hex::encode(signature.as_ref()),
                public_key: hex::encode(public_key.as_ref()),
                address_version,
            },
        });
    }

    Ok(CreateTransaction {
        inputs,
        outputs,
        druid_info: DruidInfo {
            druid: druid.to_string(),
            participants: 2,
            expectations: vec![this_expectation],
            genesis_hash: None,
        },
    })
}

/// Derives the address used to correlate the two halves of a DRUID trade
/// from a transaction's inputs: `hex(sha3_256(joined per-input P2PKH script
/// strings))`.
///
/// Rebuilds each input's real P2PKH script via [`Script::pay2pkh`] -- the
/// same construction the one-way path uses -- then delegates to
/// `tw_chain`'s own `construct_tx_ins_address`, so this is byte-for-byte the
/// same string sdk-go/sdk-js/sdk-php produce by hand from the script parts.
pub fn construct_tx_ins_address(inputs: &[CreateTxIn]) -> Result<String> {
    let mut tx_ins = Vec::with_capacity(inputs.len());
    for input in inputs {
        let CreateTxInScript::Pay2PkH {
            signable_data,
            signature,
            public_key,
            address_version,
        } = &input.script_signature;

        let signature_bytes = hex::decode(signature).map_err(|e| Error::Tx(e.to_string()))?;
        let signature = sign::Signature::from_slice(&signature_bytes)
            .ok_or_else(|| Error::Tx("invalid signature".into()))?;

        let public_key_bytes = hex::decode(public_key).map_err(|e| Error::Tx(e.to_string()))?;
        let public_key = sign::PublicKey::from_slice(&public_key_bytes)
            .ok_or_else(|| Error::Tx("invalid public key".into()))?;

        tx_ins.push(TxIn {
            previous_out: Some(input.previous_out.clone()),
            script_signature: Script::pay2pkh(
                signable_data.clone(),
                signature,
                public_key,
                *address_version,
            ),
        });
    }

    Ok(tw_construct_tx_ins_address(&tx_ins))
}

//! Typed response models for read endpoints. Sprawling payloads (full blocks,
//! blockchain entries) are surfaced as `serde_json::Value` by the client.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use tw_chain::primitives::asset::Asset;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Supply {
    pub total: u64,
    pub issued: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct OutPointRef {
    pub n: u32,
    pub t_hash: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Utxo {
    pub out_point: OutPointRef,
    pub value: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BalanceTotals {
    pub tokens: u64,
    #[serde(default)]
    pub items: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Balances {
    pub address_list: BTreeMap<String, Vec<Utxo>>,
    pub total: BalanceTotals,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BalancesResponse {
    pub balance: Balances,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TxStatus {
    pub status: String,
    pub timestamp: i64,
    pub additional_info: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DebugData {
    pub node_type: String,
    pub node_api: Vec<String>,
    pub node_peers: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PaymentAccepted {
    pub to_address: String,
    pub tx_hash: Option<String>,
    pub amount: serde_json::Value,
}

/// One party's expectation within a two-way (DRUID) trade: the asset owed,
/// who owes it, and to whom.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DruidExpectation {
    pub from: String,
    pub to: String,
    pub asset: Asset,
}

/// The DDE metadata attached to one half of a two-way trade. Unsigned:
/// never folded into any signable preimage.
///
/// `genesis_hash` is omitted at construction time (`create_2w_tx_half`
/// leaves it `None`, and it's skipped from the serialized JSON entirely);
/// the node fills it in at submission time.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DruidInfo {
    pub druid: String,
    pub participants: usize,
    pub expectations: Vec<DruidExpectation>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub genesis_hash: Option<String>,
}

/// Lifecycle status of a two-way (DRUID) trade as tracked on the valence
/// mailbox, mirroring sdk-go's `Pending2WTxStatus` / sdk-js's
/// `IPending2WTxDetails['status']`.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Pending2WTxStatus {
    /// An offer awaiting the counterparty's response.
    Pending,
    /// An offer the counterparty has accepted.
    Accepted,
    /// An offer the counterparty has rejected.
    Rejected,
}

/// The payload stored under a valence mailbox entry for a two-way (DRUID)
/// trade: both parties' expectations, the trade's current status, and the
/// mempool host the initiating sender chose (so both parties submit their
/// halves to the same node's DRUID pool). Exchanged with valence in
/// PLAINTEXT -- never encrypted on the wire. Field names are camelCase to
/// match sdk-go/sdk-js/sdk-php's wire shape byte-for-byte (see
/// `tests/fixtures/twoway.json`'s `pending2WTxDetailsOffer`).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Pending2WTxDetails {
    pub druid: String,
    #[serde(rename = "senderExpectation")]
    pub sender_expectation: DruidExpectation,
    #[serde(rename = "receiverExpectation")]
    pub receiver_expectation: DruidExpectation,
    pub status: Pending2WTxStatus,
    #[serde(rename = "mempoolHost")]
    pub mempool_host: String,
}

/// A passphrase-encrypted [`crate::druid::CreateTransaction`] half, sealed
/// under the wallet's own keystore master key by
/// [`crate::wallet::Wallet::encrypt_transaction`]. Never sent to valence --
/// this is purely the caller-persisted at-rest form.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct EncryptedTransaction {
    pub druid: String,
    /// Hex-encoded `nonce || ciphertext` blob (see [`crate::wallet::crypto`]).
    pub blob: String,
}

/// The caller-persisted record of a two-way payment this wallet initiated
/// via `make_2way_payment`: the DRUID correlating the trade, this party's
/// half sealed at rest, and both parties' expectations exactly as posted to
/// valence (so a later `fetch_pending_2way_payment` call can match its own
/// stored half back up against the mailbox contents).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PendingHalf {
    pub druid: String,
    pub encrypted_half: EncryptedTransaction,
    pub sender_expectation: DruidExpectation,
    pub receiver_expectation: DruidExpectation,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deserializes_supply() {
        let s: Supply = serde_json::from_str(r#"{"total":360360000000000000,"issued":90103919694881008}"#).unwrap();
        assert_eq!(s.total, 360_360_000_000_000_000);
        assert_eq!(s.issued, 90_103_919_694_881_008);
    }

    #[test]
    fn supply_serializes_back_to_json() {
        let s = Supply { total: 360_360_000_000_000_000, issued: 42 };
        let v = serde_json::to_value(&s).unwrap();
        assert_eq!(v, serde_json::json!({"total": 360_360_000_000_000_000u64, "issued": 42}));
    }

    #[test]
    fn deserializes_balances_response() {
        let body = r#"{"balance":{"address_list":{"a1":[{"out_point":{"n":0,"t_hash":"g59"},"value":{"Token":720720000}}]},"total":{"tokens":720720000,"items":{}}}}"#;
        let r: BalancesResponse = serde_json::from_str(body).unwrap();
        assert_eq!(r.balance.total.tokens, 720_720_000);
        assert_eq!(r.balance.address_list["a1"][0].out_point.t_hash, "g59");
    }

    #[test]
    fn deserializes_debug_data() {
        let body = r#"{"node_type":"Mempool","node_api":["v1/debug","v1/supply"],"node_peers":[]}"#;
        let d: DebugData = serde_json::from_str(body).unwrap();
        assert_eq!(d.node_type, "Mempool");
        assert_eq!(d.node_api.len(), 2);
    }
}

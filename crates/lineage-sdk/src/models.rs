//! Typed response models for read endpoints. Sprawling payloads (full blocks,
//! blockchain entries) are surfaced as `serde_json::Value` by the client.

use std::collections::BTreeMap;

use serde::Deserialize;

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct Supply {
    pub total: u64,
    pub issued: u64,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct OutPointRef {
    pub n: u32,
    pub t_hash: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Utxo {
    pub out_point: OutPointRef,
    pub value: serde_json::Value,
}

#[derive(Debug, Clone, Deserialize)]
pub struct BalanceTotals {
    pub tokens: u64,
    #[serde(default)]
    pub items: serde_json::Value,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Balances {
    pub address_list: BTreeMap<String, Vec<Utxo>>,
    pub total: BalanceTotals,
}

#[derive(Debug, Clone, Deserialize)]
pub struct BalancesResponse {
    pub balance: Balances,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct TxStatus {
    pub status: String,
    pub timestamp: i64,
    pub additional_info: String,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct DebugData {
    pub node_type: String,
    pub node_api: Vec<String>,
    pub node_peers: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct PaymentAccepted {
    pub to_address: String,
    pub tx_hash: Option<String>,
    pub amount: serde_json::Value,
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

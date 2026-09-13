//! Two-way (DDE) trade construction, verified byte-for-byte against the
//! shared cross-SDK vector in `tests/fixtures/twoway.json` (also used by
//! sdk-go, sdk-php, and sdk-js).

use std::collections::HashMap;

use lineage_sdk::{
    construct_tx_ins_address, create_2w_tx_half, generate_druid, CreateTxIn, DruidExpectation,
    FetchBalanceResponse, KeyPairs,
};
use tw_chain::crypto::sign_ed25519 as sign;

fn fixture() -> serde_json::Value {
    let raw = include_str!("fixtures/twoway.json");
    serde_json::from_str(raw).expect("valid fixture JSON")
}

/// Rebuilds a ring-compatible PKCS8 v2 Ed25519 document from the raw
/// 32-byte seed + 32-byte public key format the shared fixture (and
/// sdk-go/sdk-python/sdk-js, which use a plain Ed25519 implementation) use
/// for secret keys. `tw_chain`'s signer only accepts PKCS8 documents, so
/// this wrapping is purely a fixture-loading concern -- it does not touch
/// the signer itself.
fn secret_key_from_fixture(secret_key_hex: &str) -> sign::SecretKey {
    let raw = hex::decode(secret_key_hex).expect("valid hex secret key");
    assert_eq!(
        raw.len(),
        64,
        "fixture secret keys are 32-byte seed + 32-byte public key"
    );
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

fn keypairs_from_fixture(fixture: &serde_json::Value) -> KeyPairs {
    let mut map = HashMap::new();
    for entry in fixture["fixedKeypairs"]["ours"].as_array().unwrap() {
        let address = entry["address"].as_str().unwrap().to_string();
        let public_key = public_key_from_hex(entry["public_key"].as_str().unwrap());
        let secret_key = secret_key_from_fixture(entry["secret_key"].as_str().unwrap());
        map.insert(address, (public_key, secret_key));
    }
    map
}

#[test]
fn create_2w_tx_half_matches_vector_byte_for_byte() {
    let fixture = fixture();
    let case = &fixture["create2WTxHalf"];
    let input = &case["input"];
    let expected = &case["output"];

    let druid = fixture["druid"].as_str().unwrap();
    let balance: FetchBalanceResponse =
        serde_json::from_value(input["fetchBalanceResponse"].clone()).expect("valid balance");
    let sender_expectation: DruidExpectation =
        serde_json::from_value(input["senderExpectation"].clone()).expect("valid expectation");
    let receiver_expectation: DruidExpectation =
        serde_json::from_value(input["receiverExpectation"].clone()).expect("valid expectation");
    let excess_address = input["excessAddress"].as_str().unwrap();
    let locktime = input["locktime"].as_u64().unwrap();

    let keypairs = keypairs_from_fixture(&fixture);

    let result = create_2w_tx_half(
        druid,
        sender_expectation,
        receiver_expectation,
        &balance,
        &keypairs,
        excess_address,
        locktime,
    )
    .expect("create_2w_tx_half should succeed against the vector");

    assert_eq!(
        serde_json::to_value(&result.druid_info).unwrap(),
        expected["druid_info"],
        "druid_info mismatch"
    );
    assert_eq!(
        serde_json::to_value(&result.outputs).unwrap(),
        expected["outputs"],
        "outputs mismatch"
    );
    assert_eq!(
        serde_json::to_value(&result.inputs).unwrap(),
        expected["inputs"],
        "inputs (incl. per-input signable_data/signature/public_key) mismatch"
    );
}

#[test]
fn construct_tx_ins_address_matches_vector() {
    let fixture = fixture();
    let case = &fixture["constructTxInsAddress"];

    let inputs: Vec<CreateTxIn> =
        serde_json::from_value(case["input"].clone()).expect("valid CreateTxIn list");
    let expected = case["address"].as_str().unwrap();

    let address = construct_tx_ins_address(&inputs).expect("construct_tx_ins_address succeeds");

    assert_eq!(address, expected);
}

#[test]
fn generate_druid_matches_expected_shape() {
    let druid = generate_druid();

    assert!(
        druid.starts_with("DRUID0x"),
        "druid {druid} missing DRUID0x prefix"
    );
    let suffix = &druid["DRUID0x".len()..];
    assert_eq!(
        suffix.len(),
        32,
        "druid {druid} suffix should be 32 hex chars"
    );
    assert!(
        suffix
            .chars()
            .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()),
        "druid {druid} suffix should be lowercase hex"
    );

    // Two calls should (overwhelmingly likely) produce different DRUIDs.
    assert_ne!(generate_druid(), druid);
}

//! Payment transaction construction, ported from fleet-core's `utils.rs`.
//!
//! The signing sequence is load-bearing: the signable hash is computed over a
//! zero-signature `TxIn` together with *all* `TxOut`s, before any signature is
//! attached. Signing anything else produces a transaction the network will reject.

use tw_chain::crypto::sign_ed25519 as sign;
use tw_chain::primitives::asset::{Asset, TokenAmount};
use tw_chain::primitives::transaction::{OutPoint, Transaction, TxIn, TxOut};
use tw_chain::script::lang::Script;
use tw_chain::utils::transaction_utils::{
    construct_tx_core, construct_tx_hash, construct_tx_in_out_signable_hash,
};

use crate::wallet::ADDRESS_VERSION;

/// A UTXO to spend, along with the keypair authorized to spend it.
pub struct SpendInput {
    pub t_hash: String,
    pub n: i32,
    pub public_key: sign::PublicKey,
    pub secret_key: sign::SecretKey,
}

/// A payment destination.
pub struct PayOutput {
    pub address: String,
    pub amount: u64,
}

/// Builds and signs a payment transaction spending `inputs` to `outputs`.
///
/// Returns the transaction hash alongside the constructed `Transaction`.
pub fn build_signed_payment(inputs: &[SpendInput], outputs: &[PayOutput]) -> (String, Transaction) {
    let tx_outs: Vec<TxOut> = outputs
        .iter()
        .map(|out| TxOut {
            value: Asset::Token(TokenAmount(out.amount)),
            locktime: 0,
            script_public_key: Some(out.address.clone()),
        })
        .collect();

    let tx_ins: Vec<TxIn> = inputs
        .iter()
        .map(|input| {
            let previous_out = Some(OutPoint::new(input.t_hash.clone(), input.n));
            let signable = TxIn {
                previous_out: previous_out.clone(),
                script_signature: Script::new(),
            };
            let signable_hash = construct_tx_in_out_signable_hash(&signable, &tx_outs);
            let signature = sign::sign_detached(signable_hash.as_bytes(), &input.secret_key);
            TxIn {
                previous_out,
                script_signature: Script::pay2pkh(
                    signable_hash,
                    signature,
                    input.public_key,
                    Some(ADDRESS_VERSION),
                ),
            }
        })
        .collect();

    let tx = construct_tx_core(tx_ins, tx_outs, None);
    let hash = construct_tx_hash(&tx);
    (hash, tx)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tw_chain::utils::transaction_utils::construct_address_for;

    #[test]
    fn built_payment_has_stable_hash_and_valid_p2pkh() {
        let (pk, sk) = sign::gen_keypair();
        let _addr = construct_address_for(&pk, Some(ADDRESS_VERSION));
        let inputs = vec![SpendInput {
            t_hash: "g0000000000000000000000000000000".into(),
            n: 0,
            public_key: pk,
            secret_key: sk,
        }];
        let outputs = vec![PayOutput {
            address: "recipient".into(),
            amount: 1000,
        }];
        let (hash, tx) = build_signed_payment(&inputs, &outputs);

        assert!(!hash.is_empty());
        assert_eq!(tx.inputs.len(), 1);
        assert_eq!(tx.outputs.len(), 1);

        // Recompute the signable hash the same way the network would: over the
        // zero-signature TxIn plus all TxOuts. The signature embedded in the
        // pay2pkh script must verify against it under the sender's public key.
        let previous_out = tx.inputs[0].previous_out.clone();
        let zero_sig_in = TxIn {
            previous_out,
            script_signature: Script::new(),
        };
        let expected_hash = construct_tx_in_out_signable_hash(&zero_sig_in, &tx.outputs);

        let signature = match &tx.inputs[0].script_signature.stack[1] {
            tw_chain::script::StackEntry::Signature(sig) => *sig,
            other => panic!("expected signature stack entry, got {other:?}"),
        };

        assert!(sign::verify_detached(
            &signature,
            expected_hash.as_bytes(),
            &pk
        ));

        // Hash construction is deterministic for the same transaction.
        assert_eq!(construct_tx_hash(&tx), hash);
    }
}

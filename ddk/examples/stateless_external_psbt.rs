//! Stateless DLC lifecycle with funding inputs signed by an external wallet.
//!
//! External signers (hardware wallets, remote services, other software) need
//! no DDK-specific code:
//!
//! ```text
//! create_funding_psbt -> serialize PSBT -> external wallet signs and
//! finalizes its own inputs -> deserialize PSBT -> sign_accept / finalize_sign
//! ```
//!
//! The lifecycle functions verify that the returned PSBT spends exactly the
//! funding transaction rebuilt from the wire messages, so a signer cannot
//! mutate outputs, locktimes, sequences, or outpoints.
//!
//! Run with `cargo run --example stateless_external_psbt`.

#[allow(dead_code)]
mod util {
    include!("common/stateless.rs");
}

use bitcoin::bip32::{DerivationPath, Xpriv};
use bitcoin::psbt::Psbt;
use bitcoin::{Amount, Network, ScriptBuf, Witness};
use ddk::contract::{
    accept_offer, create_funding_psbt, create_offer, finalize_sign, sign_accept, signing,
    AcceptOfferParams,
};
use ddk_dlc::secp256k1_zkp::{All, Secp256k1};
use util::PartySetup;

fn main() {
    let secp = Secp256k1::new();
    let network = Network::Regtest;

    let offerer = PartySetup::new(&secp, 1, network, Amount::from_sat(150_000));
    // The accept party's UTXO lives in an "external wallet" that only speaks PSBT.
    let accepter = PartySetup::new(&secp, 2, network, Amount::from_sat(150_000));

    let offer = create_offer(util::offer_params(
        &secp,
        &offerer,
        Amount::from_sat(50_000),
        network,
    ))
    .expect("valid offer");

    let accept_result = accept_offer(
        &offer,
        AcceptOfferParams {
            party: accepter.party_params(&secp),
            min_timeout_interval: 100,
            max_timeout_interval: 500,
        },
        &accepter.funding_secret_key,
    )
    .expect("valid accept");
    let accept = accept_result.accept;

    // The offer party signs with whatever source it prefers (xpriv here).
    let mut offer_psbt = create_funding_psbt(&offer, &accept).expect("funding psbt");
    signing::sign_funding_psbt_with_xpriv(
        &offer,
        &accept,
        &mut offer_psbt,
        &offerer.xpriv,
        &offerer.derivations(),
    )
    .expect("offer xpriv signing");
    let sign_result =
        sign_accept(&offer, &accept, &offerer.funding_secret_key, &offer_psbt).expect("sign");

    // The accept party serializes the PSBT and hands it to the external wallet.
    let psbt = create_funding_psbt(&offer, &accept).expect("funding psbt");
    let serialized = psbt.serialize();
    let returned_bytes = external_wallet_sign(
        serialized,
        &accepter.xpriv,
        &accepter.derivation_path,
        &secp,
    );
    let returned = Psbt::deserialize(&returned_bytes).expect("external wallet returned a PSBT");

    // Inputs belonging to the offer party are still unsigned in `returned`;
    // finalize_sign only requires the accept party's inputs to be finalized.
    let funding_transaction =
        finalize_sign(&offer, &accept, &sign_result.sign, &returned).expect("finalize");

    println!(
        "completed funding transaction {} with {} signed inputs",
        funding_transaction.compute_txid(),
        funding_transaction.input.len()
    );
}

/// Stands in for an external wallet: signs and finalizes only the inputs it
/// owns, using nothing but rust-bitcoin.
fn external_wallet_sign(
    serialized_psbt: Vec<u8>,
    xpriv: &Xpriv,
    path: &DerivationPath,
    secp: &Secp256k1<All>,
) -> Vec<u8> {
    let mut psbt = Psbt::deserialize(&serialized_psbt).unwrap();
    let private_key = xpriv.derive_priv(secp, path).unwrap().to_priv();
    let public_key = private_key.public_key(secp);
    let owned_script = ScriptBuf::new_p2wpkh(&public_key.wpubkey_hash().unwrap());
    let fingerprint = xpriv.fingerprint(secp);

    for input in &mut psbt.inputs {
        let owns_input = input
            .witness_utxo
            .as_ref()
            .map(|utxo| utxo.script_pubkey == owned_script)
            .unwrap_or(false);
        if owns_input {
            input
                .bip32_derivation
                .insert(public_key.inner, (fingerprint, path.clone()));
        }
    }
    psbt.sign(xpriv, secp).unwrap();
    for input in &mut psbt.inputs {
        let Some((public_key, signature)) = input
            .partial_sigs
            .iter()
            .map(|(pk, sig)| (*pk, *sig))
            .next()
        else {
            continue;
        };
        input.final_script_witness = Some(Witness::from_slice(&[
            signature.to_vec(),
            public_key.to_bytes(),
        ]));
        input.partial_sigs.clear();
    }
    psbt.serialize()
}

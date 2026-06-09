//! Stateless DLC lifecycle with funding inputs signed by a private output
//! descriptor.
//!
//! The lifecycle is identical to `stateless_xpriv`; only the signing source
//! changes. Watch-only descriptors are rejected, and only `wpkh()` and
//! `sh(wpkh())` descriptors are supported.
//!
//! Run with `cargo run --example stateless_descriptor`.

#[allow(dead_code)]
mod util {
    include!("common/stateless.rs");
}

use bitcoin::{Amount, Network};
use ddk::contract::{
    accept_offer, create_funding_psbt, create_offer, finalize_sign, sign_accept, signing,
    AcceptOfferParams, DescriptorInput,
};
use ddk_dlc::secp256k1_zkp::Secp256k1;
use util::PartySetup;

fn main() {
    let secp = Secp256k1::new();
    let network = Network::Regtest;

    let offerer = PartySetup::new(&secp, 1, network, Amount::from_sat(150_000));
    let accepter = PartySetup::new(&secp, 2, network, Amount::from_sat(150_000));

    // Private wildcard descriptors over the same BIP84 tree the UTXOs use.
    let offer_descriptor = format!("wpkh({}/84h/1h/0h/0/*)", offerer.xpriv);
    let accept_descriptor = format!("wpkh({}/84h/1h/0h/0/*)", accepter.xpriv);

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

    // Offer party signs with its descriptor; inputs are addressed by funding
    // input serial id plus the descriptor's wildcard index.
    let mut offer_psbt = create_funding_psbt(&offer, &accept).expect("funding psbt");
    signing::sign_funding_psbt_with_descriptor(
        &offer,
        &accept,
        &mut offer_psbt,
        &offer_descriptor,
        &[DescriptorInput {
            input_serial_id: offerer.funding_input.input_serial_id,
            derivation_index: 0,
        }],
    )
    .expect("offer descriptor signing");
    let sign_result =
        sign_accept(&offer, &accept, &offerer.funding_secret_key, &offer_psbt).expect("sign");

    let mut accept_psbt = create_funding_psbt(&offer, &accept).expect("funding psbt");
    signing::sign_funding_psbt_with_descriptor(
        &offer,
        &accept,
        &mut accept_psbt,
        &accept_descriptor,
        &[DescriptorInput {
            input_serial_id: accepter.funding_input.input_serial_id,
            derivation_index: 0,
        }],
    )
    .expect("accept descriptor signing");
    let funding_transaction =
        finalize_sign(&offer, &accept, &sign_result.sign, &accept_psbt).expect("finalize");

    println!(
        "completed funding transaction {} with {} signed inputs",
        funding_transaction.compute_txid(),
        funding_transaction.input.len()
    );
}

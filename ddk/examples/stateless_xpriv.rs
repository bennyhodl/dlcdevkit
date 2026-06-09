//! Stateless DLC lifecycle with funding inputs signed by a raw BIP32 xpriv.
//!
//! ```text
//! create_offer -> accept_offer -> create_funding_psbt
//!   -> signing::sign_funding_psbt_with_xpriv (both parties)
//!   -> sign_accept -> finalize_sign -> broadcast (caller's chain client)
//! ```
//!
//! Run with `cargo run --example stateless_xpriv`.

#[allow(dead_code)]
mod util {
    include!("common/stateless.rs");
}

use bitcoin::{Amount, Network};
use ddk::contract::{
    accept_offer, create_funding_psbt, create_offer, finalize_sign, sign_accept, signing,
    AcceptOfferParams,
};
use ddk_dlc::secp256k1_zkp::Secp256k1;
use util::PartySetup;

fn main() {
    let secp = Secp256k1::new();
    let network = Network::Regtest;

    // Each party holds a DLC funding key and a wallet xpriv with one UTXO.
    let offerer = PartySetup::new(&secp, 1, network, Amount::from_sat(150_000));
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

    // Offer party: sign its funding input with the xpriv and create the sign message.
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

    // Accept party: sign its funding input and complete the funding transaction.
    let mut accept_psbt = create_funding_psbt(&offer, &accept).expect("funding psbt");
    signing::sign_funding_psbt_with_xpriv(
        &offer,
        &accept,
        &mut accept_psbt,
        &accepter.xpriv,
        &accepter.derivations(),
    )
    .expect("accept xpriv signing");
    let funding_transaction =
        finalize_sign(&offer, &accept, &sign_result.sign, &accept_psbt).expect("finalize");

    // Broadcasting stays with the caller, e.g. `chain.send_transaction(&funding_transaction)`.
    println!(
        "completed funding transaction {} with {} signed inputs",
        funding_transaction.compute_txid(),
        funding_transaction.input.len()
    );
}

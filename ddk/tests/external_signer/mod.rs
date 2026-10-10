//! A signer outside DDK, for the tests of [`ddk::contract::external`].
//!
//! It answers the signing requests with a party's keys using nothing but
//! rust-bitcoin and secp256k1-zkp: the PSBTs in a request carry everything a
//! signer needs, so no DDK code touches a secret key. A vault, an HSM or a
//! hardware wallet does the same on its side of the boundary.

#![allow(dead_code)]

use bitcoin::psbt::Psbt;
use bitcoin::secp256k1::Message;
use bitcoin::sighash::SighashCache;
use ddk::contract::external::{ContractSignatures, ContractSigningRequest, FundingSigningRequest};
use ddk_dlc::secp256k1_zkp::{EcdsaAdaptorSignature, Secp256k1, SecretKey};

/// Signs the refund and encrypts a signature of each CET to its adaptor point,
/// with the contract funding key.
pub fn sign_contract(
    request: &ContractSigningRequest,
    funding_secret_key: &SecretKey,
) -> ContractSignatures {
    let secp = Secp256k1::new();
    let cet_sighashes: Vec<Message> = request.cets.iter().map(|cet| sighash(cet, 0)).collect();
    ContractSignatures {
        refund_signature: bitcoin::ecdsa::Signature::sighash_all(
            secp.sign_ecdsa(&sighash(&request.refund, 0), funding_secret_key),
        ),
        cet_adaptor_signatures: request
            .adaptor_points
            .iter()
            .map(|(cet_index, adaptor_point)| {
                EcdsaAdaptorSignature::encrypt(
                    &secp,
                    &cet_sighashes[*cet_index],
                    funding_secret_key,
                    adaptor_point,
                )
            })
            .collect(),
    }
}

/// Leaves this party's half of each splice input's 2-of-2 in `psbt`: a
/// partial signature by its funding key of the previous contract.
pub fn sign_splice_inputs(
    request: &FundingSigningRequest,
    prior_funding_secret_key: &SecretKey,
    psbt: &mut Psbt,
) {
    let secp = Secp256k1::new();
    for splice in &request.splice_inputs {
        let signature = secp.sign_ecdsa(
            &sighash(&request.psbt, splice.input_index),
            prior_funding_secret_key,
        );
        psbt.inputs[splice.input_index].partial_sigs.insert(
            bitcoin::PublicKey::new(splice.funding_pubkey),
            bitcoin::ecdsa::Signature::sighash_all(signature),
        );
    }
}

/// The sighash of a PSBT input, from the PSBT alone.
fn sighash(psbt: &Psbt, input_index: usize) -> Message {
    psbt.sighash_ecdsa(input_index, &mut SighashCache::new(&psbt.unsigned_tx))
        .expect("a request PSBT carries what its sighash needs")
        .0
}

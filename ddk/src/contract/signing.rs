//! Funding sources for signing the funding PSBT.
//!
//! Every funding source produces finalized witnesses inside the funding PSBT;
//! the lifecycle functions ([`sign_accept`](super::sign_accept) and
//! [`finalize_sign`](super::finalize_sign)) then extract those witnesses. The
//! core DLC algorithms never branch on where a signature came from.
//!
//! | Source | Function | Notes |
//! |--------|----------|-------|
//! | DDK wallet | [`sign_funding_psbt_with_wallet`] | Any [`ddk_manager::Wallet`] implementation |
//! | Raw xpriv | [`sign_funding_psbt_with_xpriv`] | Caller supplies BIP32 paths per input |
//! | Private descriptor | [`sign_funding_psbt_with_descriptor`] | `wpkh()` and `sh(wpkh())` descriptors |
//! | External signer | none required | Serialize the PSBT, sign elsewhere, deserialize |
//!
//! External signers need no DDK-specific code: serialize the PSBT produced by
//! [`create_funding_psbt`](super::create_funding_psbt), let the external
//! wallet sign and finalize its own inputs, then pass the PSBT back to the
//! lifecycle functions. Inputs belonging to the other party may remain
//! unsigned.
//!
//! Inputs are identified by funding input serial id, so any subset of inputs
//! can be signed regardless of transaction position or which party owns them.

use bdk_wallet::miniscript::descriptor::{
    Descriptor, DescriptorPublicKey, DescriptorSecretKey, KeyMap, ShInner, Wildcard,
};
use bitcoin::bip32::{ChildNumber, Xpriv};
use bitcoin::psbt::Psbt;
use bitcoin::sighash::SighashCache;
use bitcoin::{NetworkKind, PrivateKey, ScriptBuf};
use ddk_dlc::secp256k1_zkp::{All, Secp256k1};
use ddk_messages::{AcceptDlc, OfferDlc};

use super::context::funding_input_index;
use super::error::ContractError;
use super::psbt::{ensure_matching_psbt, finalize_segwit_input};
use super::types::{network_from_chain_hash, DescriptorInput, InputDerivation, Party};

/// Signs and finalizes one party's funding inputs with a wallet.
///
/// Works with any [`ddk_manager::Wallet`] implementation capable of signing
/// PSBT inputs, such as [`crate::wallet::DlcDevKitWallet`]. The wallet only
/// sees the funding PSBT; no manager, signer provider, or storage trait is
/// involved.
pub async fn sign_funding_psbt_with_wallet<W>(
    offer: &OfferDlc,
    accept: &AcceptDlc,
    psbt: &mut Psbt,
    wallet: &W,
    party: Party,
) -> Result<(), ContractError>
where
    W: ddk_manager::Wallet + ?Sized,
{
    ensure_matching_psbt(offer, accept, psbt)?;
    let inputs = match party {
        Party::Offer => &offer.funding_inputs,
        Party::Accept => &accept.funding_inputs,
    };
    for input in inputs {
        let input_index = funding_input_index(offer, accept, input.input_serial_id)?;
        wallet
            .sign_psbt_input(psbt, input_index)
            .await
            .map_err(|e| ContractError::Wallet(e.to_string()))?;
        if psbt.inputs[input_index].final_script_witness.is_none() {
            finalize_segwit_input(psbt, input_index).map_err(|e| match e {
                ContractError::InvalidFundingInput(_) => ContractError::Wallet(format!(
                    "the wallet did not produce a signature for input {input_index}"
                )),
                other => other,
            })?;
        }
    }
    Ok(())
}

/// Signs and finalizes funding inputs with a BIP32 extended private key.
///
/// Each [`InputDerivation`] names a funding input by serial id and the path,
/// relative to `xpriv`, of the key controlling it. Inputs not listed are left
/// untouched. Native P2WPKH and P2SH-P2WPKH inputs are supported.
pub fn sign_funding_psbt_with_xpriv(
    offer: &OfferDlc,
    accept: &AcceptDlc,
    psbt: &mut Psbt,
    xpriv: &Xpriv,
    derivations: &[InputDerivation],
) -> Result<(), ContractError> {
    ensure_matching_psbt(offer, accept, psbt)?;
    let secp = Secp256k1::new();
    for derivation in derivations {
        let input_index = funding_input_index(offer, accept, derivation.input_serial_id)?;
        let derived = xpriv.derive_priv(&secp, &derivation.derivation_path)?;
        sign_input_with_key(psbt, input_index, &derived.to_priv(), &secp)?;
    }
    Ok(())
}

/// Signs and finalizes funding inputs with a private output descriptor.
///
/// `wpkh()` and `sh(wpkh())` descriptors are supported, with or without a
/// wildcard; each [`DescriptorInput`] names a funding input by serial id and
/// the wildcard derivation index of its script. Watch-only descriptors (no
/// private keys) and multipath descriptors are rejected. The descriptor key
/// network is validated against the offer's chain hash when the chain is
/// recognized.
pub fn sign_funding_psbt_with_descriptor(
    offer: &OfferDlc,
    accept: &AcceptDlc,
    psbt: &mut Psbt,
    descriptor: &str,
    inputs: &[DescriptorInput],
) -> Result<(), ContractError> {
    let secp = Secp256k1::new();
    let (descriptor, key_map) =
        Descriptor::<DescriptorPublicKey>::parse_descriptor(&secp, descriptor)
            .map_err(|e| ContractError::Descriptor(e.to_string()))?;
    if key_map.is_empty() {
        return Err(ContractError::Descriptor(
            "watch-only descriptor: signing requires a descriptor with private keys".to_string(),
        ));
    }
    match &descriptor {
        Descriptor::Wpkh(_) => {}
        Descriptor::Sh(sh) if matches!(sh.as_inner(), ShInner::Wpkh(_)) => {}
        _ => {
            return Err(ContractError::Descriptor(
                "only wpkh() and sh(wpkh()) descriptors are supported".to_string(),
            ))
        }
    }
    if let Some(network) = network_from_chain_hash(offer.chain_hash) {
        let network_kind = NetworkKind::from(network);
        for secret in key_map.values() {
            if let DescriptorSecretKey::XPrv(xkey) = secret {
                if xkey.xkey.network != network_kind {
                    return Err(ContractError::Descriptor(format!(
                        "descriptor key network does not match the offer chain ({network})"
                    )));
                }
            }
        }
    }
    ensure_matching_psbt(offer, accept, psbt)?;

    for input in inputs {
        let input_index = funding_input_index(offer, accept, input.input_serial_id)?;
        let definite = descriptor
            .at_derivation_index(input.derivation_index)
            .map_err(|e| ContractError::Descriptor(e.to_string()))?;
        let expected_script_pubkey = definite.script_pubkey();
        let input_script_pubkey = psbt.inputs[input_index]
            .witness_utxo
            .as_ref()
            .map(|utxo| utxo.script_pubkey.clone())
            .ok_or_else(|| {
                ContractError::PsbtMismatch(format!(
                    "PSBT input {input_index} is missing its witness UTXO"
                ))
            })?;
        if expected_script_pubkey != input_script_pubkey {
            return Err(ContractError::Descriptor(format!(
                "descriptor does not derive the script of input serial id {} at index {}",
                input.input_serial_id, input.derivation_index
            )));
        }
        let private_key = derive_descriptor_private_key(
            &key_map,
            input.derivation_index,
            &expected_script_pubkey,
            &secp,
        )
        .ok_or_else(|| {
            ContractError::Descriptor(format!(
                "descriptor private keys do not derive the script of input serial id {}",
                input.input_serial_id
            ))
        })?;
        sign_input_with_key(psbt, input_index, &private_key, &secp)?;
    }
    Ok(())
}

fn derive_descriptor_private_key(
    key_map: &KeyMap,
    derivation_index: u32,
    expected_script_pubkey: &ScriptBuf,
    secp: &Secp256k1<All>,
) -> Option<PrivateKey> {
    key_map
        .values()
        .filter_map(|secret| candidate_private_key(secret, derivation_index, secp))
        .find(|candidate| {
            let Ok(hash) = candidate.public_key(secp).wpubkey_hash() else {
                return false;
            };
            let native = ScriptBuf::new_p2wpkh(&hash);
            *expected_script_pubkey == native
                || *expected_script_pubkey == ScriptBuf::new_p2sh(&native.script_hash())
        })
}

fn candidate_private_key(
    secret: &DescriptorSecretKey,
    derivation_index: u32,
    secp: &Secp256k1<All>,
) -> Option<PrivateKey> {
    match secret {
        DescriptorSecretKey::Single(single) => Some(single.key),
        DescriptorSecretKey::XPrv(xkey) => {
            let path = match xkey.wildcard {
                Wildcard::None => xkey.derivation_path.clone(),
                Wildcard::Unhardened => xkey
                    .derivation_path
                    .child(ChildNumber::from_normal_idx(derivation_index).ok()?),
                Wildcard::Hardened => xkey
                    .derivation_path
                    .child(ChildNumber::from_hardened_idx(derivation_index).ok()?),
            };
            Some(xkey.xkey.derive_priv(secp, &path).ok()?.to_priv())
        }
        DescriptorSecretKey::MultiXPrv(_) => None,
    }
}

/// Signs one P2WPKH or P2SH-P2WPKH PSBT input with a concrete key and
/// finalizes it.
fn sign_input_with_key(
    psbt: &mut Psbt,
    input_index: usize,
    private_key: &PrivateKey,
    secp: &Secp256k1<All>,
) -> Result<(), ContractError> {
    let public_key = private_key.public_key(secp);
    let wpubkey_hash = public_key.wpubkey_hash().map_err(|_| {
        ContractError::InvalidFundingInput(format!(
            "input {input_index} cannot be signed with an uncompressed key"
        ))
    })?;
    let input = psbt.inputs.get(input_index).ok_or_else(|| {
        ContractError::PsbtMismatch(format!("PSBT input {input_index} does not exist"))
    })?;
    let script_pubkey = input
        .witness_utxo
        .as_ref()
        .map(|utxo| utxo.script_pubkey.clone())
        .ok_or_else(|| {
            ContractError::PsbtMismatch(format!(
                "PSBT input {input_index} is missing its witness UTXO"
            ))
        })?;

    let native = ScriptBuf::new_p2wpkh(&wpubkey_hash);
    let controls_input = if script_pubkey.is_p2wpkh() {
        script_pubkey == native
    } else if script_pubkey.is_p2sh() {
        input.redeem_script.as_ref() == Some(&native)
    } else {
        return Err(ContractError::UnsupportedScriptType { input_index });
    };
    if !controls_input {
        return Err(ContractError::InvalidFundingInput(format!(
            "the derived key does not control the script of input {input_index}; \
             check the derivation path or index"
        )));
    }

    let (message, sighash_type) = {
        let mut cache = SighashCache::new(&psbt.unsigned_tx);
        psbt.sighash_ecdsa(input_index, &mut cache).map_err(|e| {
            ContractError::InvalidFundingInput(format!(
                "could not compute the sighash for input {input_index}: {e}"
            ))
        })?
    };
    let signature = bitcoin::ecdsa::Signature {
        signature: secp.sign_ecdsa(&message, &private_key.inner),
        sighash_type,
    };
    psbt.inputs[input_index]
        .partial_sigs
        .insert(public_key, signature);
    finalize_segwit_input(psbt, input_index)
}

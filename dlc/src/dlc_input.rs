//! Module for working with DLC inputs

use bitcoin::{Amount, EcdsaSighashType, OutPoint, ScriptBuf, Transaction, Witness};
use secp256k1_zkp::{PublicKey, Secp256k1, SecretKey, Signing, Verification};

use crate::util::{finalize_sig, multisig_witness, sighash_all_signature};
use crate::{Error, TxInputInfo};

#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "use-serde", derive(serde::Serialize, serde::Deserialize))]
/// Contains information about a DLC input to be used in a funding transaction.
pub struct DlcInputInfo {
    /// The transaction of the funding transaction.
    pub fund_tx: Transaction,
    /// The output index of the funding transaction.
    pub fund_vout: u32,
    /// The local funding public key.
    pub local_fund_pubkey: PublicKey,
    /// The remote funding public key.
    pub remote_fund_pubkey: PublicKey,
    /// The amount of the funding transaction.
    pub fund_amount: Amount,
    /// The maximum witness length of the funding transaction.
    pub max_witness_len: usize,
    /// The serial id of the funding transaction.
    pub input_serial_id: u64,
    /// The contract id referenced
    pub contract_id: [u8; 32],
}

impl From<&DlcInputInfo> for TxInputInfo {
    fn from(val: &DlcInputInfo) -> Self {
        TxInputInfo {
            outpoint: OutPoint::new(val.fund_tx.compute_txid(), val.fund_vout),
            max_witness_len: val.max_witness_len,
            redeem_script: ScriptBuf::new(),
            serial_id: val.input_serial_id,
        }
    }
}

/// Calculate weight of DLC inputs for fee estimation
pub fn get_dlc_inputs_weight(dlc_inputs: &[DlcInputInfo]) -> usize {
    dlc_inputs
        .iter()
        .map(|dlc_input| {
            // P2WSH 2-of-2 multisig weight
            36 * 4 + 4 + 4 * 4 + dlc_input.max_witness_len
        })
        .sum()
}

/// Calculate total amount from DLC inputs
pub fn calculate_total_dlc_input_amount(dlc_inputs: &[DlcInputInfo]) -> Amount {
    dlc_inputs.iter().map(|input| input.fund_amount).sum()
}

/// Create the funding script for a DLC input
pub fn create_dlc_input_funding_script(dlc_input: &DlcInputInfo) -> ScriptBuf {
    crate::make_funding_redeemscript(&dlc_input.local_fund_pubkey, &dlc_input.remote_fund_pubkey)
}

/// Create a signature for a DLC funding input
pub fn create_dlc_funding_input_signature<C: Signing>(
    secp: &Secp256k1<C>,
    fund_transaction: &Transaction,
    input_index: usize,
    dlc_input: &DlcInputInfo,
    privkey: &SecretKey,
) -> Result<Vec<u8>, Error> {
    let funding_script = create_dlc_input_funding_script(dlc_input);
    let sig_hash_msg = super::util::get_sig_hash_msg(
        fund_transaction,
        input_index,
        &funding_script,
        dlc_input.fund_amount,
    )?;
    let signature = secp.sign_ecdsa_low_r(&sig_hash_msg, privkey);
    Ok(finalize_sig(&signature, EcdsaSighashType::All))
}

/// Verify a DLC funding input signature
///
/// `signature` is one party's half of the 2-of-2 witness as it goes on chain:
/// a DER signature followed by the `SIGHASH_ALL` byte, as produced by
/// [`create_dlc_funding_input_signature`]. Any other encoding or sighash type
/// is rejected, since it could not be combined into a valid witness.
///
/// Changed in 2.0.0: a 64-byte compact signature, or a DER signature with a
/// sighash byte other than `SIGHASH_ALL`, used to be accepted. Callers that
/// hold a raw [`Signature`](secp256k1_zkp::ecdsa::Signature) pass
/// `util::finalize_sig(&signature, EcdsaSighashType::All)` instead.
pub fn verify_dlc_funding_input_signature<V: Verification>(
    secp: &Secp256k1<V>,
    fund_transaction: &Transaction,
    input_index: usize,
    dlc_input: &DlcInputInfo,
    signature: Vec<u8>,
    pubkey: &PublicKey,
) -> Result<(), Error> {
    let funding_script = create_dlc_input_funding_script(dlc_input);
    let signature = sighash_all_signature(&signature)?;

    super::verify_tx_input_sig(
        secp,
        &signature,
        fund_transaction,
        input_index,
        &funding_script,
        dlc_input.fund_amount,
        pubkey,
    )
}

/// Combine both parties' signatures for a DLC input
///
/// Both signatures are encoded as [`create_dlc_funding_input_signature`]
/// produces them (DER followed by `SIGHASH_ALL`); anything else is an error
/// rather than a witness that fails on chain. The witness is built by
/// [`multisig_witness`](crate::util::multisig_witness).
///
/// Changed in 2.0.0: the signatures are `&[u8]` (was `&Vec<u8>`, which still
/// coerces, so call sites compile unchanged) and the result is
/// `Result<Witness, Error>` (was `Witness`): add `?` at the call site.
pub fn combine_dlc_input_signatures(
    dlc_input: &DlcInputInfo,
    my_signature: &[u8],
    other_signature: &[u8],
    my_pubkey: &PublicKey,
    other_pubkey: &PublicKey,
) -> Result<Witness, Error> {
    Ok(multisig_witness(
        &create_dlc_input_funding_script(dlc_input),
        (*my_pubkey, sighash_all_signature(my_signature)?),
        (*other_pubkey, sighash_all_signature(other_signature)?),
    ))
}

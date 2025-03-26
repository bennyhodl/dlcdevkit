use bitcoin::opcodes::all::*;
use bitcoin::taproot::LeafVersion;
use bitcoin::TapNodeHash;
use bitcoin::{
    absolute::LockTime, script::Builder, transaction::Version, Amount, ScriptBuf, Transaction,
    TxOut, XOnlyPublicKey,
};
use dlc::{DlcTransactions, Payout, TxInputInfo};
use secp256k1_zkp::{rand, All, Keypair, Secp256k1};
use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Error, Debug, Clone)]
enum TaprootDlcError {
    #[error("Not a taproor script pubkey")]
    NotTaproot,
    #[error("Adaptor Signature is not valid.")]
    InvalidAdaptorSignature,
    #[error("Numeric contracts not supported")]
    NumericContract,
    #[error("Secp error")]
    Secp,
    #[error("Generating Address")]
    GetAddress,
    #[error("{0}")]
    General(String),
    #[error("Esplora skill issue.")]
    Esplora,
    #[error("Oracle error")]
    Oracle,
}

/// Contains the parameters required for creating DLC transactions for a single
/// party. Specifically these are the common fields between Offer and Accept
/// messages.
#[derive(Clone, Debug)]
#[cfg_attr(
    feature = "use-serde",
    derive(Serialize, Deserialize),
    serde(rename_all = "camelCase")
)]
pub struct TaprootPartyParams {
    /// The public key for the fund multisig script
    pub fund_pubkey: XOnlyPublicKey,
    /// An address to receive change
    pub change_script_pubkey: ScriptBuf,
    /// Id used to order fund outputs
    pub change_serial_id: u64,
    /// An address to receive the outcome amount
    pub payout_script_pubkey: ScriptBuf,
    /// Id used to order CET outputs
    pub payout_serial_id: u64,
    /// A list of inputs to fund the contract
    pub inputs: Vec<TxInputInfo>,
    /// The sum of the inputs values.
    pub input_amount: Amount,
    /// The collateral put in the contract by the party
    pub collateral: Amount,
}

fn create_taproot_dlc_transactions(
    secp: &Secp256k1<All>,
    offer_params: &TaprootPartyParams,
    accept_params: &TaprootPartyParams,
    payouts: &[Payout],
    refund_lock_time: u32,
    fee_rate_per_vb: u64,
    fund_lock_time: u32,
    cet_lock_time: u32,
    fund_output_serial_id: u64,
) -> Result<DlcTransactions, TaprootDlcError> {
    let (funding_transaction, funding_script) = create_funding_transaction(
        secp,
        &offer_params.fund_pubkey,
        &accept_params.fund_pubkey,
        accept_params.collateral,
        offer_params.collateral,
    )?;

    todo!()
}

fn create_funding_transaction(
    secp: &Secp256k1<All>,
    offer_pubkey: &XOnlyPublicKey,
    accept_pubkey: &XOnlyPublicKey,
    accept: Amount,
    offer: Amount,
) -> Result<(Transaction, ScriptBuf), TaprootDlcError> {
    let funding_script = create_funding_script(secp, offer_pubkey, accept_pubkey)?;

    let transaction = Transaction {
        version: Version::TWO,
        lock_time: LockTime::ZERO,
        input: vec![],
        output: vec![TxOut {
            value: accept + offer,
            script_pubkey: funding_script.clone(),
        }],
    };

    Ok((transaction, funding_script))
}

fn create_funding_script(
    secp: &Secp256k1<All>,
    offer_pubkey: &XOnlyPublicKey,
    accept_pubkey: &XOnlyPublicKey,
) -> Result<ScriptBuf, TaprootDlcError> {
    // Can use the serial ordering in rust-dlc insteaf
    let (first_pubkey, second_pubkey) = if offer_pubkey < accept_pubkey {
        (offer_pubkey, accept_pubkey)
    } else {
        (accept_pubkey, offer_pubkey)
    };

    let script_spend = Builder::new()
        .push_x_only_key(&first_pubkey)
        .push_opcode(OP_CHECKSIG)
        .push_x_only_key(&second_pubkey)
        .push_opcode(OP_CHECKSIGADD)
        .push_int(2)
        .push_opcode(OP_NUMEQUALVERIFY)
        .into_script();

    let tap_tree = TapNodeHash::from_script(script_spend.as_script(), LeafVersion::TapScript);

    // Does this need to be stored? Or is it just a throwaway key?
    let internal_key = Keypair::new(secp, &mut rand::thread_rng());
    let internal_pubkey = internal_key.x_only_public_key().0;

    Ok(ScriptBuf::new_p2tr(&secp, internal_pubkey, Some(tap_tree)))
}

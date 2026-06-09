//! Parameter and result types for the stateless contract API.

use bitcoin::blockdata::constants::ChainHash;
use bitcoin::key::rand::{thread_rng, Rng};
use bitcoin::psbt::Psbt;
use bitcoin::{Amount, Network, ScriptBuf, Transaction};
use ddk_dlc::secp256k1_zkp::PublicKey;
use ddk_dlc::DlcTransactions;
use ddk_messages::contract_msgs::ContractInfo;
use ddk_messages::{AcceptDlc, FundingInput, SignDlc};

use super::error::ContractError;

/// Identifies which party's funding inputs an operation applies to.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Party {
    /// The party that created the offer.
    Offer,
    /// The party that accepted the offer.
    Accept,
}

/// One party's Bitcoin-level contract data.
///
/// The funding public key is the DLC funding key used for the multisig funding
/// output, adaptor signatures, and the refund signature. It is distinct from
/// the keys controlling `funding_inputs`, which are regular wallet UTXOs and
/// are signed through the PSBT signing layer.
#[derive(Clone, Debug)]
pub struct PartyParams {
    /// The DLC funding public key of this party.
    pub funding_pubkey: PublicKey,
    /// The wallet UTXOs this party contributes to the funding transaction.
    pub funding_inputs: Vec<FundingInput>,
    /// The script pubkey CET and refund payouts are sent to.
    pub payout_spk: ScriptBuf,
    /// Serial id ordering the payout output. Randomly generated when `None`.
    pub payout_serial_id: Option<u64>,
    /// The script pubkey funding change is sent to.
    pub change_spk: ScriptBuf,
    /// Serial id ordering the change output. Randomly generated when `None`.
    pub change_serial_id: Option<u64>,
}

/// Parameters for [`create_offer`](super::create_offer).
#[derive(Clone, Debug)]
pub struct CreateOfferParams {
    /// The chain the contract settles on. See [`chain_hash_from_network`].
    pub chain_hash: [u8; 32],
    /// Identifies the contract before it is funded. Randomly generated when `None`.
    pub temporary_contract_id: Option<[u8; 32]>,
    /// The contract payout and oracle information.
    pub contract_info: ContractInfo,
    /// The collateral contributed by the offering party.
    pub offer_collateral: Amount,
    /// The offering party's Bitcoin-level contract data.
    pub party: PartyParams,
    /// Serial id ordering the funding output. Randomly generated when `None`.
    pub fund_output_serial_id: Option<u64>,
    /// The fee rate, in satoshis per virtual byte, for the funding transaction and CETs.
    pub fee_rate_per_vb: u64,
    /// The earliest time CETs can be broadcast.
    pub cet_locktime: u32,
    /// The time after which the refund transaction can be broadcast.
    pub refund_locktime: u32,
    /// Contract feature flags. Use `0` unless a protocol extension requires otherwise.
    pub contract_flags: u8,
}

/// Parameters for [`accept_offer`](super::accept_offer).
#[derive(Clone, Debug)]
pub struct AcceptOfferParams {
    /// The accepting party's Bitcoin-level contract data.
    ///
    /// `party.funding_pubkey` must match the public key of the DLC funding
    /// secret key passed to [`accept_offer`](super::accept_offer).
    pub party: PartyParams,
    /// The minimum accepted interval between the oracle event maturity and the
    /// refund locktime.
    pub min_timeout_interval: u32,
    /// The maximum accepted interval between the oracle event maturity and the
    /// refund locktime.
    pub max_timeout_interval: u32,
}

/// The result of [`accept_offer`](super::accept_offer).
///
/// This is an operation result, not persisted contract state. The accept
/// message is the authoritative artifact; the transactions and PSBT can be
/// deterministically rebuilt from the offer and accept messages at any time.
pub struct AcceptResult {
    /// The accept message to send to the offering party.
    pub accept: AcceptDlc,
    /// The unsigned funding, CET, and refund transactions.
    pub transactions: DlcTransactions,
    /// The funding PSBT ready to be signed by either party's funding source.
    pub funding_psbt: Psbt,
}

/// The result of [`sign_accept`](super::sign_accept).
pub struct SignResult {
    /// The sign message to send to the accepting party.
    pub sign: SignDlc,
    /// The unsigned funding, CET, and refund transactions.
    pub transactions: DlcTransactions,
}

/// Identifies a funding input and the BIP32 path that derives its key.
///
/// Inputs are identified by their funding input serial id, not by transaction
/// position, so derivations remain stable regardless of input ordering.
#[derive(Clone, Debug)]
pub struct InputDerivation {
    /// The serial id of the funding input to sign.
    pub input_serial_id: u64,
    /// The derivation path of the key controlling the input, relative to the
    /// extended private key passed to
    /// [`sign_funding_psbt_with_xpriv`](super::signing::sign_funding_psbt_with_xpriv).
    pub derivation_path: bitcoin::bip32::DerivationPath,
}

/// Identifies a funding input and the descriptor derivation index for its script.
#[derive(Clone, Debug)]
pub struct DescriptorInput {
    /// The serial id of the funding input to sign.
    pub input_serial_id: u64,
    /// The wildcard derivation index of the input's script. Ignored for
    /// descriptors without a wildcard.
    pub derivation_index: u32,
}

/// Creates a funding input from a previous transaction and output index.
///
/// A random serial id is generated when `input_serial_id` is `None`. For
/// P2SH-wrapped SegWit inputs, `redeem_script` must contain the witness
/// program; for native SegWit inputs it must be empty.
pub fn funding_input(
    previous_transaction: &Transaction,
    vout: u32,
    input_serial_id: Option<u64>,
    sequence: u32,
    max_witness_len: u16,
    redeem_script: ScriptBuf,
) -> Result<FundingInput, ContractError> {
    let prevout = previous_transaction
        .output
        .get(vout as usize)
        .ok_or_else(|| {
            ContractError::InvalidFundingInput(format!("previous output {vout} does not exist"))
        })?;
    if prevout.script_pubkey.is_p2sh() {
        if redeem_script.is_empty() {
            return Err(ContractError::InvalidFundingInput(
                "P2SH input requires a redeem script".to_string(),
            ));
        }
        if ScriptBuf::new_p2sh(&redeem_script.script_hash()) != prevout.script_pubkey {
            return Err(ContractError::InvalidFundingInput(
                "redeem script does not match the P2SH script pubkey".to_string(),
            ));
        }
    } else if !redeem_script.is_empty() {
        return Err(ContractError::InvalidFundingInput(
            "redeem script provided for a non-P2SH input".to_string(),
        ));
    }
    Ok(FundingInput {
        input_serial_id: input_serial_id.unwrap_or_else(random_serial_id),
        prev_tx: bitcoin::consensus::serialize(previous_transaction),
        prev_tx_vout: vout,
        sequence,
        max_witness_len,
        redeem_script,
        dlc_input: None,
    })
}

/// Returns the DLC chain hash for a network, suitable for
/// [`CreateOfferParams::chain_hash`].
pub fn chain_hash_from_network(network: Network) -> [u8; 32] {
    ChainHash::using_genesis_block_const(network).to_bytes()
}

pub(crate) fn network_from_chain_hash(chain_hash: [u8; 32]) -> Option<Network> {
    [
        Network::Bitcoin,
        Network::Testnet,
        Network::Signet,
        Network::Regtest,
    ]
    .into_iter()
    .find(|network| chain_hash_from_network(*network) == chain_hash)
}

pub(crate) fn random_serial_id() -> u64 {
    thread_rng().gen()
}

pub(crate) fn random_temporary_contract_id() -> [u8; 32] {
    let mut id = [0u8; 32];
    thread_rng().fill(&mut id);
    id
}

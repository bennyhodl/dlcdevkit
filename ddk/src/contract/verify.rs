//! Verification of a signed contract from its three wire messages.

use ddk_dlc::secp256k1_zkp::Secp256k1;
use ddk_dlc::DlcTransactions;
use ddk_messages::{AcceptDlc, OfferDlc, SignDlc};

use super::context::{signed_context, verify_offer_funding_signatures, verify_party_signatures};
use super::error::ContractError;

/// A contract whose offer, accept and sign messages were checked against each
/// other by [`verify_signed_contract`].
///
/// Holding one means every signature the messages carry is valid for the
/// transactions it exposes, except the offer funding witnesses listed by
/// [`unverified_funding_inputs`](Self::unverified_funding_inputs), whose script
/// type DDK cannot check. Code that imports, settles or splices the contract
/// can use the transactions without checking them again.
pub struct VerifiedContract {
    contract_id: [u8; 32],
    transactions: DlcTransactions,
    unverified_funding_inputs: Vec<u64>,
}

impl VerifiedContract {
    /// Serial ids of the offering party's funding inputs whose witnesses were
    /// not checked, because they spend a script type DDK cannot verify (see
    /// [`ddk_dlc::verify_funding_witness`]).
    ///
    /// Such a witness is neither accepted nor rejected: if it is bad, the
    /// funding transaction simply does not confirm. A consumer that must know
    /// every signature is valid before it proceeds requires this to be empty.
    pub fn unverified_funding_inputs(&self) -> &[u64] {
        &self.unverified_funding_inputs
    }

    /// The contract id named by the sign message, which the rebuilt funding
    /// transaction matches.
    pub fn contract_id(&self) -> [u8; 32] {
        self.contract_id
    }

    /// The contract's transactions. The funding transaction is unsigned: the
    /// accepting party's funding witnesses are not part of the messages.
    pub fn transactions(&self) -> &DlcTransactions {
        &self.transactions
    }

    /// Takes the contract's transactions.
    pub fn into_transactions(self) -> DlcTransactions {
        self.transactions
    }
}

/// Verifies that the offer, accept and sign messages describe one signed
/// contract and returns its transactions.
///
/// The transactions are rebuilt under the fee rule whose funding transaction
/// matches the sign message's contract id, as in
/// [`create_signed_dlc_transactions`](super::create_signed_dlc_transactions),
/// so contracts signed before the current rule verify too. Then every
/// signature the messages carry is checked against them:
///
/// - both parties' refund signatures and CET adaptor signatures (the accept
///   message carries the accepting party's, the sign message the offering
///   party's);
/// - the offering party's funding witnesses from the sign message: each
///   ordinary input's witness against the output it spends (P2WPKH,
///   P2SH-P2WPKH or a Taproot key-path spend, signing with `SIGHASH_ALL`), and
///   each splice input's half signature against the previous contract's
///   offer-side funding key.
///
/// An offer funding input of any other script type (a P2WSH multisig, for
/// example, which an external signer can fund from) does not fail
/// verification, because DDK cannot tell whether its witness is valid. It is
/// listed in [`VerifiedContract::unverified_funding_inputs`] instead, and the
/// chain is its final check.
///
/// The accepting party's funding witnesses, and its halves of any splice
/// inputs, never travel in the messages; it adds them in
/// [`finalize_sign`](super::finalize_sign). They cannot be checked here.
///
/// Failures are attributed to the message that carried the bad data:
/// [`ContractError::InvalidAccept`] for the accept message's signatures and
/// [`ContractError::InvalidSign`] for the sign message's.
pub fn verify_signed_contract(
    offer: &OfferDlc,
    accept: &AcceptDlc,
    sign: &SignDlc,
) -> Result<VerifiedContract, ContractError> {
    let context = signed_context(offer, accept, sign)?;
    let secp = Secp256k1::new();
    let total_collateral = offer.get_total_collateral();
    verify_party_signatures(
        &secp,
        &context,
        total_collateral,
        accept.funding_pubkey,
        &accept.refund_signature,
        &accept.cet_adaptor_signatures,
        ContractError::InvalidAccept,
    )?;
    verify_party_signatures(
        &secp,
        &context,
        total_collateral,
        offer.funding_pubkey,
        &sign.refund_signature,
        &sign.cet_adaptor_signatures,
        ContractError::InvalidSign,
    )?;
    let unverified_funding_inputs = verify_offer_funding_signatures(
        &secp,
        offer,
        accept,
        &context.transactions.fund,
        &sign.funding_signatures,
    )?;
    Ok(VerifiedContract {
        contract_id: sign.contract_id,
        transactions: context.transactions,
        unverified_funding_inputs,
    })
}

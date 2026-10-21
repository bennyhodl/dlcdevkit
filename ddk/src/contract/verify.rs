//! Verification of a signed contract from its three wire messages.

use ddk_dlc::secp256k1_zkp::Secp256k1;
use ddk_dlc::DlcTransactions;
use ddk_messages::{AcceptDlc, OfferDlc, SignDlc};

use super::context::{signed_context, verify_offer_funding_signatures, verify_party_signatures};
use super::error::ContractError;

/// A contract whose offer, accept and sign messages were checked against each
/// other by [`verify_signed_contract`].
///
/// Holding one means every signature the three messages carry is valid for the
/// transactions it exposes. Code that imports, settles or splices the contract
/// can use the transactions without checking them again.
///
/// The accepting party's funding witnesses are not among those signatures:
/// they are not in the messages, so the funding transaction exposed here is
/// unsigned.
pub struct VerifiedContract {
    contract_id: [u8; 32],
    transactions: DlcTransactions,
}

impl VerifiedContract {
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
/// **The accepting party's funding witnesses are not verified here.** They,
/// and its halves of any splice inputs, never travel in the three messages:
/// the accepting party adds them in [`finalize_sign`](super::finalize_sign).
/// This function verifies what the messages carry, all of it, and a success
/// means none of it is left unchecked.
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
///   P2SH-P2WPKH or a Taproot key-path spend, signing with `SIGHASH_ALL`, and
///   no longer than the input's declared `max_witness_len`), and each splice
///   input's half signature against the previous contract's offer-side funding
///   key.
///
/// An offer funding input of any other script type (a P2WSH multisig, for
/// example) fails with [`ContractError::UnverifiableFundingWitness`]: DDK
/// cannot tell whether its witness is valid, so the contract cannot be
/// verified. A witness longer than its input declared fails with
/// [`ContractError::FundingWitnessTooLong`], since the funding fee was paid on
/// the declared length. Other failures are attributed to the message that
/// carried the bad data: [`ContractError::InvalidAccept`] for the accept
/// message's signatures and [`ContractError::InvalidSign`] for the sign
/// message's.
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
    verify_offer_funding_signatures(
        &secp,
        offer,
        accept,
        &context.transactions.fund,
        &sign.funding_signatures,
    )?;
    Ok(VerifiedContract {
        contract_id: sign.contract_id,
        transactions: context.transactions,
    })
}

//! # AcceptedContract

use super::offered_contract::OfferedContract;
use super::utils::cet_ranges;
use super::AdaptorInfo;
use crate::error::Error;
use bitcoin::{Amount, SignedAmount, Transaction};
use ddk_dlc::{DlcTransactions, PartyParams};
use ddk_messages::tlv_stream::TlvStream;
use ddk_messages::{AcceptDlc, FundingInput};
use secp256k1_zkp::ecdsa::Signature;
use secp256k1_zkp::EcdsaAdaptorSignature;

use std::fmt::Write as _;

/// An AcceptedContract represents a contract in the accepted state.
#[derive(Clone)]
pub struct AcceptedContract {
    /// The offered contract that was accepted.
    pub offered_contract: OfferedContract,
    /// The parameters of the accepting party.
    pub accept_params: PartyParams,
    /// The funding inputs provided by the accepting party.
    pub funding_inputs: Vec<FundingInput>,
    /// The adaptor information for the contract storing information about
    /// the relation between adaptor signatures and outcomes.
    pub adaptor_infos: Vec<AdaptorInfo>,
    /// The adaptor signatures of the accepting party. Note that the accepting
    /// party does not keep them thus an option is used.
    pub adaptor_signatures: Vec<EcdsaAdaptorSignature>,
    /// The signature for the refund transaction from the accepting party.
    pub accept_refund_signature: Signature,
    /// The bitcoin set of bitcoin transactions for the contract.
    pub dlc_transactions: DlcTransactions,
    /// The TLV records from the accept message. Persisted by the storage layer
    /// suffix, not by this struct's `Writeable` (see `ddk::util::ser`).
    pub tlvs: TlvStream,
}

impl AcceptedContract {
    /// Where the CETs and the adaptor signatures of the contract info at
    /// `index` start in `dlc_transactions.cets` and in the contract's adaptor
    /// signature list. Both hold every contract info's entries one after
    /// another, and a contract info's own indexes count from its start.
    pub(crate) fn execution_offsets(&self, index: usize) -> Result<(usize, usize), Error> {
        let offered = &self.offered_contract;
        let cet_start = cet_ranges(&offered.contract_info, offered.total_collateral)?[index].start;
        let adaptor_start = offered
            .contract_info
            .iter()
            .zip(&self.adaptor_infos)
            .take(index)
            .map(|(contract_info, adaptor_info)| {
                contract_info.adaptor_signature_count(adaptor_info)
            })
            .sum();
        Ok((cet_start, adaptor_start))
    }

    /// Returns the contract id for the contract computed as specified here:
    /// <https://github.com/discreetlogcontracts/dlcspecs/blob/master/Protocol.md#requirements-2>
    pub fn get_contract_id(&self) -> [u8; 32] {
        crate::utils::compute_id(
            self.dlc_transactions.fund.compute_txid(),
            self.dlc_transactions.get_fund_output_index() as u16,
            &self.offered_contract.id,
        )
    }

    /// Utility function to get the contract id as a string.
    pub fn get_contract_id_string(&self) -> String {
        let mut string_id = String::with_capacity(32 * 2 + 2);
        string_id.push_str("0x");
        let id = self.get_contract_id();
        for i in &id {
            write!(string_id, "{i:02x}").unwrap();
        }

        string_id
    }

    /// Construct the accept contract message
    pub fn get_accept_contract_msg(
        &self,
        ecdsa_adaptor_signatures: &[EcdsaAdaptorSignature],
    ) -> AcceptDlc {
        AcceptDlc {
            protocol_version: crate::conversion_utils::PROTOCOL_VERSION,
            temporary_contract_id: self.offered_contract.id,
            accept_collateral: self.accept_params.collateral,
            funding_pubkey: self.accept_params.fund_pubkey,
            payout_spk: self.accept_params.payout_script_pubkey.clone(),
            payout_serial_id: self.accept_params.payout_serial_id,
            funding_inputs: self.funding_inputs.clone(),
            change_spk: self.accept_params.change_script_pubkey.clone(),
            change_serial_id: self.accept_params.change_serial_id,
            cet_adaptor_signatures: ecdsa_adaptor_signatures.into(),
            refund_signature: self.accept_refund_signature,
            negotiation_fields: None,
            tlvs: self.tlvs.clone(),
        }
    }

    /// Compute the profit and loss for this contract and an assciated cet index
    pub fn compute_pnl(&self, cet: &Transaction) -> SignedAmount {
        let offer = &self.offered_contract;
        let party_params = if offer.is_offer_party {
            &offer.offer_params
        } else {
            &self.accept_params
        };
        let collateral = party_params.collateral;
        let v0_witness_payout_script = &party_params.payout_script_pubkey;
        let final_payout = cet
            .output
            .iter()
            .find_map(|x| {
                if &x.script_pubkey == v0_witness_payout_script {
                    Some(x.value)
                } else {
                    None
                }
            })
            .unwrap_or(Amount::ZERO);
        SignedAmount::from_sat(final_payout.to_sat() as i64 - collateral.to_sat() as i64)
    }
}

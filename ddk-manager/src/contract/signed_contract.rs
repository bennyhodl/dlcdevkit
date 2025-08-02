//! #SignedContract

use crate::conversion_utils::PROTOCOL_VERSION;
use crate::utils::get_new_serial_id;
use crate::ChannelId;

use super::accepted_contract::AcceptedContract;
use dlc::dlc_input::DlcInputInfo;
use dlc_messages::CetAdaptorSignature;
use dlc_messages::CetAdaptorSignatures;
use dlc_messages::FundingSignatures;
use dlc_messages::SignDlc;
use secp256k1_zkp::ecdsa::Signature;
use secp256k1_zkp::EcdsaAdaptorSignature;

/// Contain information about a contract that was fully signed.
#[derive(Clone)]
pub struct SignedContract {
    /// The accepted contract that was signed.
    pub accepted_contract: AcceptedContract,
    /// The adaptor signatures of the offering party (None if offering party).
    pub adaptor_signatures: Option<Vec<EcdsaAdaptorSignature>>,
    /// The refund signature of the offering party.
    pub offer_refund_signature: Signature,
    /// The signatures for the funding inputs of the offering party.
    pub funding_signatures: FundingSignatures,
    /// The [`ChannelId`] to which the contract was associated if any.
    pub channel_id: Option<ChannelId>,
}

impl SignedContract {
    pub(crate) fn get_sign_dlc(
        &self,
        cet_adaptor_signatures: Vec<EcdsaAdaptorSignature>,
    ) -> SignDlc {
        let contract_id = self.accepted_contract.get_contract_id();

        SignDlc {
            protocol_version: PROTOCOL_VERSION,
            contract_id,
            cet_adaptor_signatures: CetAdaptorSignatures {
                ecdsa_adaptor_signatures: cet_adaptor_signatures
                    .into_iter()
                    .map(|x| CetAdaptorSignature { signature: x })
                    .collect(),
            },
            refund_signature: self.offer_refund_signature,
            funding_signatures: self.funding_signatures.clone(),
        }
    }

    /// Use an existing contract to create a funding input for a spliced DLC.
    pub(crate) fn get_dlc_input(&self) -> DlcInputInfo {
        let fund_tx = self.accepted_contract.dlc_transactions.fund.clone();

        let fund_vout = self
            .accepted_contract
            .dlc_transactions
            .get_fund_output_index() as u32;
        let local_fund_pubkey = self
            .accepted_contract
            .offered_contract
            .offer_params
            .fund_pubkey;
        let remote_fund_pubkey = self.accepted_contract.accept_params.fund_pubkey;
        let fund_amount = self
            .accepted_contract
            .dlc_transactions
            .get_fund_output()
            .value;

        DlcInputInfo {
            fund_tx,
            fund_vout,
            local_fund_pubkey,
            remote_fund_pubkey,
            fund_amount,
            max_witness_len: 220,
            input_serial_id: get_new_serial_id(),
            contract_id: self.accepted_contract.get_contract_id(),
        }
    }
}

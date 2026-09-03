//! #OfferedContract

use crate::conversion_utils::{
    get_contract_info_and_announcements, get_tx_input_infos, LEGACY_CHAINHASH, PROTOCOL_VERSION,
};
use crate::dlc_input::get_dlc_inputs_from_funding_inputs;
use crate::utils::get_new_serial_id;

use super::contract_info::ContractInfo;
use super::contract_input::ContractInput;
use super::ContractDescriptor;
use crate::{ContractId, KeysId};
use bitcoin::Amount;
use ddk_dlc::PartyParams;
use ddk_messages::oracle_msgs::OracleAnnouncement;
use ddk_messages::{FundingInput, OfferDlc};
use secp256k1_zkp::PublicKey;

/// Contains information about a contract that was offered.
#[derive(Clone, Debug)]
#[cfg_attr(
    feature = "use-serde",
    derive(serde::Serialize, serde::Deserialize),
    serde(rename_all = "camelCase")
)]
pub struct OfferedContract {
    /// The temporary id of the contract.
    pub id: [u8; 32],
    /// Indicated whether the contract was proposed or received.
    pub is_offer_party: bool,
    /// The set of contract information that are used to generate CET and
    /// adaptor signatures.
    pub contract_info: Vec<ContractInfo>,
    /// The public key of the counter-party's node.
    pub counter_party: PublicKey,
    /// The parameters of the offering party.
    pub offer_params: PartyParams,
    /// The sum of both parties collateral.
    pub total_collateral: Amount,
    /// Information about the offering party's funding inputs.
    pub funding_inputs: Vec<FundingInput>,
    /// The serial id of the fund output used for output ordering.
    pub fund_output_serial_id: u64,
    /// The fee rate to be used to construct the DLC transactions.
    pub fee_rate_per_vb: u64,
    /// The time at which the contract is expected to be closeable.
    pub cet_locktime: u32,
    /// The time at which the contract becomes refundable.
    pub refund_locktime: u32,
    /// Feature flags for the contract (bit 0: refund to accepter).
    #[cfg_attr(feature = "use-serde", serde(default))]
    pub contract_flags: u8,
    /// The genesis block hash of the chain the contract settles on, in DLC
    /// message byte order, as it appeared on the offer message.
    ///
    /// [`None`] for contracts stored before ddk tracked this. Rebuilding an
    /// offer message from those contracts uses the same genesis hash the old
    /// conversion always wrote (regtest), so the message bytes stay the same.
    #[cfg_attr(feature = "use-serde", serde(default))]
    pub chain_hash: Option<[u8; 32]>,
    /// Keys Id for generating the signers
    pub(crate) keys_id: KeysId,
}

impl OfferedContract {
    /// Validate that the contract info covers all the possible outcomes that
    /// can be attested by the oracle(s).
    pub fn validate(&self) -> Result<(), crate::error::Error> {
        ddk_dlc::util::validate_fee_rate(self.fee_rate_per_vb).map_err(|_| {
            crate::error::Error::InvalidParameters("Fee rate is too high".to_string())
        })?;

        for info in &self.contract_info {
            info.validate()?;
            let payouts = match &info.contract_descriptor {
                ContractDescriptor::Enum(e) => e.get_payouts(),
                ContractDescriptor::Numerical(e) => e.get_payouts(self.total_collateral)?,
            };
            let valid = payouts
                .iter()
                .all(|p| p.accept + p.offer == self.total_collateral);
            if !valid {
                return Err(crate::error::Error::InvalidParameters(
                    "Sum of payout doesn't equal total collateral".to_string(),
                ));
            }
        }

        Ok(())
    }

    /// Creates a new [`OfferedContract`] from the given parameters.
    ///
    /// The CET locktime is pinned to the closest oracle event maturity so that
    /// CETs are spendable exactly when the first event matures, never before.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        id: ContractId,
        contract: &ContractInput,
        oracle_announcements: Vec<Vec<OracleAnnouncement>>,
        offer_params: &PartyParams,
        funding_inputs: &[FundingInput],
        counter_party: &PublicKey,
        refund_delay: u32,
        keys_id: KeysId,
        chain_hash: [u8; 32],
    ) -> Self {
        let total_collateral = contract.offer_collateral + contract.accept_collateral;

        assert_eq!(contract.contract_infos.len(), oracle_announcements.len());

        let latest_maturity = crate::utils::get_latest_maturity_date(&oracle_announcements)
            .expect("to be able to retrieve latest maturity date");
        let cet_locktime = crate::utils::get_closest_maturity_date(&oracle_announcements)
            .expect("to be able to retrieve closest maturity date");

        let fund_output_serial_id = get_new_serial_id();
        let contract_info = contract
            .contract_infos
            .iter()
            .zip(oracle_announcements)
            .map(|(x, y)| ContractInfo {
                contract_descriptor: x.contract_descriptor.clone(),
                oracle_announcements: y,
                threshold: x.oracles.threshold as usize,
            })
            .collect::<Vec<ContractInfo>>();
        OfferedContract {
            id,
            is_offer_party: true,
            contract_info,
            offer_params: offer_params.clone(),
            total_collateral,
            funding_inputs: funding_inputs.to_vec(),
            fund_output_serial_id,
            fee_rate_per_vb: contract.fee_rate,
            cet_locktime,
            refund_locktime: latest_maturity + refund_delay,
            contract_flags: contract.contract_flags,
            chain_hash: Some(chain_hash),
            counter_party: *counter_party,
            keys_id,
        }
    }

    /// Convert an [`OfferDlc`] message to an [`OfferedContract`].
    pub fn try_from_offer_dlc(
        offer_dlc: &OfferDlc,
        counter_party: PublicKey,
        keys_id: KeysId,
    ) -> Result<OfferedContract, crate::conversion_utils::Error> {
        let contract_info = get_contract_info_and_announcements(&offer_dlc.contract_info)?;

        let (inputs, input_amount) = get_tx_input_infos(&offer_dlc.funding_inputs)?;
        let dlc_inputs = get_dlc_inputs_from_funding_inputs(&offer_dlc.funding_inputs);

        Ok(OfferedContract {
            id: offer_dlc.temporary_contract_id,
            is_offer_party: false,
            contract_info,
            offer_params: PartyParams {
                fund_pubkey: offer_dlc.funding_pubkey,
                change_script_pubkey: offer_dlc.change_spk.clone(),
                change_serial_id: offer_dlc.change_serial_id,
                payout_script_pubkey: offer_dlc.payout_spk.clone(),
                payout_serial_id: offer_dlc.payout_serial_id,
                collateral: offer_dlc.offer_collateral,
                inputs,
                dlc_inputs,
                input_amount,
            },
            cet_locktime: offer_dlc.cet_locktime,
            refund_locktime: offer_dlc.refund_locktime,
            fee_rate_per_vb: offer_dlc.fee_rate_per_vb,
            fund_output_serial_id: offer_dlc.fund_output_serial_id,
            funding_inputs: offer_dlc.funding_inputs.clone(),
            total_collateral: offer_dlc.contract_info.get_total_collateral(),
            contract_flags: offer_dlc.contract_flags,
            chain_hash: Some(offer_dlc.chain_hash),
            counter_party,
            keys_id,
        })
    }

    /// The chain hash to put on offer messages for this contract.
    ///
    /// Contracts stored before ddk tracked the chain hash have none to
    /// report, so this returns the genesis hash the old conversion always
    /// wrote.
    pub(crate) fn offer_chain_hash(&self) -> [u8; 32] {
        self.chain_hash.unwrap_or(LEGACY_CHAINHASH)
    }
}

impl From<&OfferedContract> for OfferDlc {
    fn from(offered_contract: &OfferedContract) -> OfferDlc {
        OfferDlc {
            protocol_version: PROTOCOL_VERSION,
            temporary_contract_id: offered_contract.id,
            contract_flags: offered_contract.contract_flags,
            chain_hash: offered_contract.offer_chain_hash(),
            contract_info: offered_contract.into(),
            funding_pubkey: offered_contract.offer_params.fund_pubkey,
            payout_spk: offered_contract.offer_params.payout_script_pubkey.clone(),
            payout_serial_id: offered_contract.offer_params.payout_serial_id,
            offer_collateral: offered_contract.offer_params.collateral,
            funding_inputs: offered_contract.funding_inputs.clone(),
            change_spk: offered_contract.offer_params.change_script_pubkey.clone(),
            change_serial_id: offered_contract.offer_params.change_serial_id,
            cet_locktime: offered_contract.cet_locktime,
            refund_locktime: offered_contract.refund_locktime,
            fee_rate_per_vb: offered_contract.fee_rate_per_vb,
            fund_output_serial_id: offered_contract.fund_output_serial_id,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::contract::chain_hash_from_network;
    use bitcoin::Network;

    fn validate_offer_test_common(input: &str) {
        let offer: OfferedContract = serde_json::from_str(input).unwrap();
        assert!(offer.validate().is_err());
    }

    fn offered_contract(chain_hash: Option<[u8; 32]>) -> OfferedContract {
        let offer_dlc: OfferDlc =
            serde_json::from_str(include_str!("../../test_inputs/offer_contract.json")).unwrap();
        let counter_party: PublicKey =
            "02e6642fd69bd211f93f7f1f36ca51a26a5290eb2dd1b0d8279a87bb0d480c8443"
                .parse()
                .unwrap();
        let mut offered =
            OfferedContract::try_from_offer_dlc(&offer_dlc, counter_party, [7u8; 32]).unwrap();
        offered.chain_hash = chain_hash;
        offered
    }

    /// The chain hash of a received offer is the one we send back out.
    #[test]
    fn stored_chain_hash_survives_the_offer_round_trip() {
        let mainnet = chain_hash_from_network(Network::Bitcoin);
        let offered = offered_contract(Some(mainnet));
        let offer: OfferDlc = (&offered).into();

        assert_eq!(offer.chain_hash, mainnet);
    }

    /// A contract stored before ddk tracked the chain hash rebuilds the offer
    /// with the same genesis hash the old conversion always wrote.
    #[test]
    fn contract_without_chain_hash_falls_back_to_the_legacy_constant() {
        let offered = offered_contract(None);
        let offer: OfferDlc = (&offered).into();

        assert_eq!(offer.chain_hash, LEGACY_CHAINHASH);
    }

    #[test]
    fn legacy_chain_hash_is_regtest_genesis() {
        assert_eq!(LEGACY_CHAINHASH, chain_hash_from_network(Network::Regtest));
    }

    #[test]
    fn offer_enum_missing_payout() {
        validate_offer_test_common(include_str!(
            "../../test_inputs/offer_enum_missing_payout.json"
        ));
    }

    #[test]
    fn offer_enum_oracle_with_diff_payout() {
        validate_offer_test_common(include_str!(
            "../../test_inputs/offer_enum_oracle_with_diff_payout.json"
        ));
    }

    #[test]
    fn offer_numerical_bad_first_payout() {
        validate_offer_test_common(include_str!(
            "../../test_inputs/offer_numerical_bad_first_payout.json"
        ));
    }

    #[test]
    fn offer_numerical_bad_last_payout() {
        validate_offer_test_common(include_str!(
            "../../test_inputs/offer_numerical_bad_last_payout.json"
        ));
    }

    #[test]
    fn offer_numerical_non_continuous() {
        validate_offer_test_common(include_str!(
            "../../test_inputs/offer_numerical_non_continuous.json"
        ));
    }

    #[test]
    fn offer_enum_collateral_not_equal_payout() {
        validate_offer_test_common(include_str!(
            "../../test_inputs/offer_enum_collateral_not_equal_payout.json"
        ));
    }

    #[test]
    fn offer_numerical_collateral_less_than_payout() {
        validate_offer_test_common(include_str!(
            "../../test_inputs/offer_numerical_collateral_less_than_payout.json"
        ));
    }

    #[test]
    fn offer_numerical_invalid_rounding_interval() {
        validate_offer_test_common(include_str!(
            "../../test_inputs/offer_numerical_invalid_rounding_interval.json"
        ));
    }

    #[test]
    fn offer_numerical_empty_rounding_interval() {
        validate_offer_test_common(include_str!(
            "../../test_inputs/offer_numerical_empty_rounding_interval.json"
        ));
    }
}

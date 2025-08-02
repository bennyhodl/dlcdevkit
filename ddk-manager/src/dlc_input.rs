//! Module for working with DLC inputs
use std::ops::Deref;

use bitcoin::Transaction;
use dlc::dlc_input::DlcInputInfo;
use dlc_messages::FundingInput;
use secp256k1_zkp::{All, Secp256k1};

use crate::{
    contract::Contract, error::Error, ContractId, ContractSigner, ContractSignerProvider, Storage,
};

// todo: definitely test
/// Get the DlcInputInfo from FundingInputs
pub fn get_dlc_inputs_from_funding_inputs(funding_inputs: &[FundingInput]) -> Vec<DlcInputInfo> {
    funding_inputs
        .iter()
        .filter(|i| i.dlc_input.is_some())
        .collect::<Vec<&FundingInput>>()
        .into_iter()
        .map(|i| i.into())
        .collect::<Vec<DlcInputInfo>>()
}

pub async fn get_signature_for_dlc_input<S: Deref, X: ContractSigner, SP: Deref>(
    secp: &Secp256k1<All>,
    funding_input: &FundingInput,
    fund_transaction: &Transaction,
    input_index: usize,
    contract_id: &ContractId,
    storage: &S,
    signer_provider: &SP,
) -> Result<Vec<u8>, Error>
where
    S::Target: Storage,
    SP::Target: ContractSignerProvider<Signer = X>,
{
    let dlc_input_info: DlcInputInfo = funding_input.into();

    let contract = storage
        .get_contract(contract_id)
        .await?
        .ok_or(Error::StorageError(
            "Contract not found to sign DLC input.".to_string(),
        ))?;

    let key_id = match contract {
        Contract::Confirmed(c) => Ok(c.accepted_contract.offered_contract.keys_id),
        _ => Err(Error::InvalidState(
            "Contract must be confirmed to sign DLC input.".to_string(),
        )),
    }?;

    let dlc_input_signer = signer_provider.derive_contract_signer(key_id)?;

    dlc::dlc_input::create_dlc_funding_input_signature(
        secp,
        fund_transaction,
        input_index,
        &dlc_input_info,
        &dlc_input_signer.get_secret_key()?,
    )
    .map_err(Error::DlcError)
}

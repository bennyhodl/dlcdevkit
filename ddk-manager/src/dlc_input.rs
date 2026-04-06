//! Module for working with DLC inputs
use std::ops::Deref;

use bitcoin::Transaction;
use ddk_dlc::dlc_input::DlcInputInfo;
use ddk_messages::FundingInput;
use secp256k1_zkp::{All, Secp256k1};

use crate::{
    contract::Contract, error::Error, ContractId, ContractSigner, ContractSignerProvider, Storage,
};

/// Check if unconfirmed (Signed) contracts are allowed for DLC input signing.
/// Set DDK_ALLOW_UNCONFIRMED_SPLICE=true to enable 0-conf splice operations.
fn allow_unconfirmed_splice() -> bool {
    std::env::var("DDK_ALLOW_UNCONFIRMED_SPLICE")
        .map(|v| v.to_lowercase() == "true" || v == "1")
        .unwrap_or(false)
}

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

    // Extract keys_id from contract based on state.
    // By default, only Confirmed contracts are allowed for DLC input signing.
    // Set DDK_ALLOW_UNCONFIRMED_SPLICE=true to also allow Signed contracts
    // (funding tx in mempool but not yet confirmed). Useful for testnets and 0-conf operations.
    let key_id = match contract {
        Contract::Confirmed(c) => Ok(c.accepted_contract.offered_contract.keys_id),
        Contract::Signed(s) if allow_unconfirmed_splice() => {
            Ok(s.accepted_contract.offered_contract.keys_id)
        }
        Contract::Signed(_) => Err(Error::InvalidState(
            "Contract must be confirmed to sign DLC input. Set DDK_ALLOW_UNCONFIRMED_SPLICE=true for 0-conf splice.".to_string(),
        )),
        _ => Err(Error::InvalidState(
            "Contract must be confirmed to sign DLC input.".to_string(),
        )),
    }?;

    let dlc_input_signer = signer_provider.derive_contract_signer(key_id)?;

    ddk_dlc::dlc_input::create_dlc_funding_input_signature(
        secp,
        fund_transaction,
        input_index,
        &dlc_input_info,
        &dlc_input_signer.get_secret_key()?,
    )
    .map_err(Error::DlcError)
}

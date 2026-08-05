//! Module for working with DLC inputs
use std::ops::Deref;

use bitcoin::Transaction;
use ddk_dlc::dlc_input::DlcInputInfo;
use ddk_messages::FundingInput;
use secp256k1_zkp::{All, Secp256k1};

use crate::{
    contract::Contract, error::Error, ContractId, ContractSigner, ContractSignerProvider, Storage,
};

/// The funding public key this node holds in the contract a DLC input spends.
///
/// A [`DlcInputInfo`] names the two keys of the 2-of-2 it spends in the order
/// the spliced contract had them: `local_fund_pubkey` belongs to whoever
/// offered that contract and `remote_fund_pubkey` to whoever accepted it.
/// Either party can offer the splice, so which of the two is ours has to be
/// resolved against our own key rather than assumed from the splice roles.
pub async fn get_fund_pubkey_for_dlc_input<S: Deref, X: ContractSigner, SP: Deref>(
    secp: &Secp256k1<All>,
    contract_id: &ContractId,
    storage: &S,
    signer_provider: &SP,
) -> Result<secp256k1_zkp::PublicKey, Error>
where
    S::Target: Storage,
    SP::Target: ContractSignerProvider<Signer = X>,
{
    let contract = storage
        .get_contract(contract_id)
        .await?
        .ok_or(Error::StorageError(
            "Contract not found to resolve DLC input keys.".to_string(),
        ))?;

    let keys_id = match contract {
        Contract::Confirmed(c) => Ok(c.accepted_contract.offered_contract.keys_id),
        _ => Err(Error::InvalidState(
            "Contract must be confirmed to resolve DLC input keys.".to_string(),
        )),
    }?;

    signer_provider
        .derive_contract_signer(keys_id)?
        .get_public_key(secp)
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

    let key_id = match contract {
        Contract::Confirmed(c) => Ok(c.accepted_contract.offered_contract.keys_id),
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

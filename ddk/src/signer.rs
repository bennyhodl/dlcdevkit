use bitcoin::secp256k1::{PublicKey, SecretKey};
use serde::{Deserialize, Serialize};

use crate::error::WalletError;

#[derive(Serialize, Deserialize, Debug)]
pub struct SignerInformation {
    pub index: u32,
    pub secret_key: SecretKey,
    pub public_key: PublicKey,
}

/// Retrieval of key material for signing DLC transactions
pub trait DeriveSigner {
    fn get_key_information(&self, key_id: [u8; 32]) -> Result<SignerInformation, WalletError>;
    fn store_derived_key_id(
        &self,
        key_id: [u8; 32],
        signer_info: SignerInformation,
    ) -> Result<(), WalletError>;
    fn get_secret_key(&self, public_key: &PublicKey) -> Result<SecretKey, WalletError>;
    fn import_address_to_storage(&self, address: &bitcoin::Address) -> Result<(), WalletError>;
}

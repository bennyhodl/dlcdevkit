use bitcoin::secp256k1::{PublicKey, SecretKey};
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Debug)]
pub struct SignerInformation {
    pub index: u32,
    pub secret_key: SecretKey,
    pub public_key: PublicKey,
}

/// Trait with contract specific information
/// 1. Storing and retrieving private keys for DLC CETs.
/// 2. Tracking contract specific addresses for counterparties.
pub trait DeriveSigner {
    type Error: std::fmt::Debug;

    fn get_key_information(&self, key_id: [u8;32]) -> Result<SignerInformation, Self::Error>;
    fn store_derived_key_id(
        &self,
        key_id: [u8; 32],
        signer_info: SignerInformation,
    ) -> Result<(), Self::Error>;
    fn get_secret_key(&self, public_key: &PublicKey) -> Result<SecretKey, Self::Error>;
    fn import_address_to_storage(&self, address: &bitcoin::Address) -> Result<(), Self::Error>;
}

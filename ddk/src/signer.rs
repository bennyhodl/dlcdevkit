use bitcoin::secp256k1::{PublicKey, SecretKey};
use nostr::Keys;
use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Error, Debug)]
pub enum SignerError {
    #[error("Error with deriving signer {0}")]
    SignerError(String),
}

#[derive(Serialize, Deserialize)]
pub struct SignerInformation {
    pub index: u32,
    pub secret_key: SecretKey,
    pub public_key: PublicKey,
}

/// Trait with contract specific information
/// 1. Storing and retrieving private keys for DLC CETs.
/// 2. Tracking contract specific addresses for counterparties.
pub trait DeriveSigner {
    // Get the child key index for a given key_id.
    fn get_index_for_key_id(&self, key_id: [u8; 32]) -> Result<u32, SignerError>;
    fn store_derived_key_id(
        &self,
        key_id: [u8; 32],
        signer_info: SignerInformation,
    ) -> Result<(), SignerError>;
    fn get_secret_key(&self, public_key: &PublicKey) -> Result<SecretKey, SignerError>;
    fn import_address_to_storage(&self, address: &bitcoin::Address) -> Result<(), SignerError>;
}

pub struct SimpleDeriveSigner {}

impl DeriveSigner for SimpleDeriveSigner {
    /// Get the index of a given key id.
    fn get_index_for_key_id(&self, _key_id: [u8; 32]) -> Result<u32, SignerError> {
        Ok(1)
    }

    /// Store the secret and public with the givem key id
    fn store_derived_key_id(
        &self,
        _key_id: [u8; 32],
        _signer_info: SignerInformation,
    ) -> Result<(), SignerError> {
        Ok(())
    }

    /// Retrieve the secrety key for a given public key.
    fn get_secret_key(&self, _public_key: &PublicKey) -> Result<SecretKey, SignerError> {
        let keys = Keys::generate();
        let secret_key = keys.secret_key().unwrap();
        let bytes = secret_key.secret_bytes();
        Ok(bitcoin::secp256k1::SecretKey::from_slice(&bytes).expect("no bytes zone!"))
    }

    fn import_address_to_storage(&self, _address: &bitcoin::Address) -> Result<(), SignerError> {
        Ok(())
    }
}

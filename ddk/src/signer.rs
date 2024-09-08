use bitcoin::{key::Secp256k1, secp256k1::{PublicKey, SecretKey}};
use bitcoin::key::rand;
use serde::{Deserialize, Serialize};

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
    type Error: std::fmt::Debug;

    // Get the child key index for a given key_id.
    fn get_index_for_key_id(&self, key_id: [u8; 32]) -> Result<u32, Self::Error>;
    fn store_derived_key_id(
        &self,
        key_id: [u8; 32],
        signer_info: SignerInformation,
    ) -> Result<(), Self::Error>;
    fn get_secret_key(&self, public_key: &PublicKey) -> Result<SecretKey, Self::Error>;
    fn import_address_to_storage(&self, address: &bitcoin::Address) -> Result<(), Self::Error>;
}

pub struct SimpleDeriveSigner {}

impl DeriveSigner for SimpleDeriveSigner {
    type Error = String;
    /// Get the index of a given key id.
    fn get_index_for_key_id(&self, _key_id: [u8; 32]) -> Result<u32, String> {
        Ok(1)
    }

    /// Store the secret and public with the givem key id
    fn store_derived_key_id(
        &self,
        _key_id: [u8; 32],
        _signer_info: SignerInformation,
    ) -> Result<(), String> {
        Ok(())
    }

    /// Retrieve the secrety key for a given public key.
    fn get_secret_key(&self, _public_key: &PublicKey) -> Result<SecretKey, String> {
        let secp = Secp256k1::new();
        let keys = bitcoin::key::Keypair::new(&secp, &mut rand::thread_rng());
        let secret_key = keys.secret_key();
        let bytes = secret_key.secret_bytes();
        Ok(bitcoin::secp256k1::SecretKey::from_slice(&bytes).expect("no bytes zone!"))
    }

    fn import_address_to_storage(&self, _address: &bitcoin::Address) -> Result<(), String> {
        Ok(())
    }
}

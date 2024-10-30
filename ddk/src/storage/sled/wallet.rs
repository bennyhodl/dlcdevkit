use super::SledStorage;
use crate::error::WalletError;
use crate::KeyStorage;
use bitcoin::secp256k1::SecretKey;

impl KeyStorage for SledStorage {
    fn get_secret_key(&self, key_id: [u8; 32]) -> Result<SecretKey, WalletError> {
        let key = hex::encode(key_id);
        let info = self.signer_tree()?.get(key)?.unwrap();
        Ok(serde_json::from_slice::<SecretKey>(&info)?)
    }

    /// Store the secret and public with the givem key id
    fn store_secret_key(&self, key_id: [u8; 32], secret_key: SecretKey) -> Result<(), WalletError> {
        let serialized_signer_info = serde_json::to_vec(&secret_key)?;

        // Store the key id string instead of bytes.
        let key_id = hex::encode(key_id);

        self.signer_tree()?.insert(key_id, serialized_signer_info)?;
        Ok(())
    }
}

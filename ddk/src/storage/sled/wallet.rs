use super::SledStorageProvider;
use crate::signer::{DeriveSigner, SignerInformation};
use bdk_wallet::ChangeSet;
use bitcoin::secp256k1::{PublicKey, SecretKey};
use crate::error::WalletError;
use bdk_wallet::WalletPersister;

impl WalletPersister for SledStorageProvider {
    type Error = WalletError;

    fn persist(_persister: &mut Self, _changeset: &ChangeSet) -> Result<(), Self::Error> {
       Ok(()) 
    }

    fn initialize(_persister: &mut Self) -> Result<ChangeSet, Self::Error> {
       Ok(ChangeSet::default()) 
    }
}

impl DeriveSigner for SledStorageProvider {
    type Error = WalletError;

    /// Get the index of a given key id.
    fn get_index_for_key_id(&self, key_id: [u8; 32]) -> Result<u32, WalletError> {
        if let Some(value) = self.signer_tree()?.get(key_id)? {
            let signer_info: SignerInformation = bincode::deserialize(&value).unwrap();
            Ok(signer_info.index)
        } else {
            let key_id = hex::encode(&key_id);
            tracing::warn!(key_id, "Value not found in sled database. Defaulting to index 1.");
            Ok(1)
        }
    }

    /// Store the secret and public with the givem key id
    fn store_derived_key_id(
        &self,
        key_id: [u8; 32],
        signer_information: SignerInformation,
    ) -> Result<(), WalletError> {
        let serialized_signer_info = bincode::serialize(&signer_information).map_err(|_| {
            WalletError::StorageError(sled::Error::Io(std::io::Error::new(
                std::io::ErrorKind::Other,
                "Deserialization error aggregating changset.",
            )))
        })?;

        self.signer_tree()?.insert(key_id, serialized_signer_info)?;
        Ok(())
    }

    /// Retrieve the secrety key for a given public key.
    fn get_secret_key(&self, public_key: &PublicKey) -> Result<SecretKey, WalletError> {
        let tree = self.signer_tree()?;
        for result in tree.iter() {
            if let Ok(value) = result {
                let info: SignerInformation = bincode::deserialize(&value.1).map_err(|_| {
                    WalletError::StorageError(sled::Error::Io(std::io::Error::new(
                        std::io::ErrorKind::Other,
                        "Deserialization error aggregating changset.",
                    )))
                })?;
                if info.public_key == *public_key {
                    return Ok(info.secret_key);
                }
            }
        }

        Err(WalletError::SignerError("Could not find secret key.".into()))
    }

    fn import_address_to_storage(&self, _address: &bitcoin::Address) -> Result<(), WalletError> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use bitcoin::key::Secp256k1;

    use crate::{signer::SignerInformation, storage::SledStorageProvider, DeriveSigner};

    #[test]
    fn index_from_key_id() {
        let path = "tests/data/dlc_storage/sleddb/index_from_key_id";
        let storage = SledStorageProvider::new(path).unwrap();
        let secp = Secp256k1::new();
        let secret_key =
            bitcoin::secp256k1::SecretKey::new(&mut bitcoin::secp256k1::rand::thread_rng());
        let signer_info = SignerInformation {
            index: 1,
            secret_key,
            public_key: secret_key.public_key(&secp),
        };

        let _ = storage.store_derived_key_id([0u8; 32], signer_info);

        let index = storage.get_index_for_key_id([0u8; 32]).unwrap();
        assert_eq!(index, 1);

        let priv_key = storage
            .get_secret_key(&secret_key.public_key(&secp))
            .unwrap();
        assert_eq!(priv_key, secret_key);
        std::fs::remove_dir_all(path).unwrap();
    }
}

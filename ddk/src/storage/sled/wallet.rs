use super::SledStorageProvider;
use crate::signer::{DeriveSigner, SignerInformation};
use bdk::wallet::ChangeSet;
use bdk_chain::{Append, PersistBackend};
use bitcoin::secp256k1::{PublicKey, SecretKey};
use crate::error::WalletError;
use rand::{thread_rng, Rng};

impl PersistBackend<ChangeSet> for SledStorageProvider {
    type WriteError = WalletError;
    type LoadError = WalletError;

    fn write_changes(&mut self, changeset: &ChangeSet) -> Result<(), Self::WriteError> {
        self.append_changeset(changeset)
    }

    fn load_from_persistence(&mut self) -> Result<Option<ChangeSet>, Self::LoadError> {
        self.aggregate_changesets()
    }
}

impl SledStorageProvider {
    /// Append a new changeset to the Sled database.
    pub fn append_changeset(&mut self, changeset: &ChangeSet) -> Result<(), WalletError> {
        // no need to write anything if changeset is empty
        if changeset.is_empty() {
            return Ok(());
        }

        let serialized = bincode::serialize(changeset).map_err(|_| {
            WalletError::StorageError(sled::Error::Io(std::io::Error::new(
                std::io::ErrorKind::Other,
                "Serialization error appending changset.",
            )))
        })?;

        let rand_key: [u8; 32] = thread_rng().gen();
        self.wallet_tree()?
            .insert(rand_key, serialized.clone())?;

        Ok(())
    }

    /// Loads all the changesets that have been stored as one giant changeset.
    pub fn aggregate_changesets(&self) -> Result<Option<ChangeSet>, WalletError> {
        let mut changeset = Option::<ChangeSet>::None;
        for next_changeset in self.wallet_tree()?.iter() {
            let next_changeset = match next_changeset {
                Ok(next_changeset) => next_changeset,
                Err(e) => return Err(WalletError::StorageError(e)),
            };
            let next_changeset = bincode::deserialize(&next_changeset.1).map_err(|_| {
                WalletError::StorageError(sled::Error::Io(std::io::Error::new(
                    std::io::ErrorKind::Other,
                    "Deserialization error aggregating changset.",
                )))
            })?;
            match &mut changeset {
                Some(changeset) => changeset.append(next_changeset),
                changeset => *changeset = Some(next_changeset),
            }
        }
        Ok(changeset)
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

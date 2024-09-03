use bdk::wallet::ChangeSet;
use bdk_chain::{Append, PersistBackend};
use bitcoin::secp256k1::{PublicKey, SecretKey};
use rand::{thread_rng, Rng};
use crate::signer::{DeriveSigner, SignerError, SignerInformation};
use super::SledStorageProvider;

impl PersistBackend<ChangeSet> for SledStorageProvider {
    type WriteError = sled::Error;
    type LoadError = sled::Error;

    fn write_changes(&mut self, changeset: &ChangeSet) -> Result<(), Self::WriteError> {
        self.append_changeset(changeset)
    }

    fn load_from_persistence(&mut self) -> Result<Option<ChangeSet>, Self::LoadError> {
        self.aggregate_changesets()
    }
}

impl SledStorageProvider {
    /// Append a new changeset to the Sled database.
    pub fn append_changeset(&mut self, changeset: &ChangeSet) -> Result<(), sled::Error> {
        // no need to write anything if changeset is empty
        if changeset.is_empty() {
            return Ok(());
        }

        let serialized = bincode::serialize(changeset)
            .map_err(|_| sled::Error::Io(std::io::Error::new(std::io::ErrorKind::Other, "Serialization error")))?;

        let rand_key: [u8; 32] = thread_rng().gen();
        self.wallet_tree().unwrap().insert(rand_key, serialized.clone())?;

        Ok(())
    }

    /// Loads all the changesets that have been stored as one giant changeset.
    pub fn aggregate_changesets(&self) -> Result<Option<ChangeSet>, sled::Error> {
        let mut changeset = Option::<ChangeSet>::None;
        for next_changeset in self.wallet_tree().unwrap().iter() {
            let next_changeset = match next_changeset {
                Ok(next_changeset) => next_changeset,
                Err(e) => {
                    return Err(e)
                }
            };
            let next_changeset = bincode::deserialize(&next_changeset.1).unwrap();
            match &mut changeset {
                Some(changeset) => changeset.append(next_changeset),
                changeset => *changeset = Some(next_changeset),
            }
        }
        Ok(changeset)
    }
}

impl DeriveSigner for SledStorageProvider {
    /// Get the index of a given key id.
    fn get_index_for_key_id(&self, key_id: [u8; 32]) -> Result<u32, SignerError> {
        let value = self.signer_tree().unwrap().get(key_id).unwrap().unwrap();
        let signer_info: SignerInformation = bincode::deserialize(&value).unwrap();
        Ok(signer_info.index)
    }

    /// Store the secret and public with the givem key id
    fn store_derived_key_id(
        &self,
        key_id: [u8; 32],
        signer_information: SignerInformation,
    ) -> Result<(), SignerError> {
        let serialized_signer_info = bincode::serialize(&signer_information).unwrap();
        self.signer_tree().unwrap().insert(key_id, serialized_signer_info).unwrap();
        Ok(())
    }

    /// Retrieve the secrety key for a given public key.
    fn get_secret_key(&self, public_key: &PublicKey) -> Result<SecretKey, SignerError> {
        let tree = self.signer_tree().unwrap();
        for result in tree.iter() {
            if let Ok(value) = result {
                let info: SignerInformation = bincode::deserialize(&value.1).unwrap();
                if info.public_key == *public_key {
                    return Ok(info.secret_key)
                }
            }
        }

        Err(SignerError::SignerError("Getting secret key".into()))
    }

    fn import_address_to_storage(&self, _address: &bitcoin::Address) -> Result<(), SignerError> {
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
        let secret_key = bitcoin::secp256k1::SecretKey::new(&mut bitcoin::secp256k1::rand::thread_rng());
        let signer_info = SignerInformation {
            index: 1,
            secret_key,
            public_key: secret_key.public_key(&secp),
        };
        
        let _ = storage.store_derived_key_id([0u8;32], signer_info);

        let index = storage.get_index_for_key_id([0u8;32]).unwrap();
        assert_eq!(index, 1);

        let priv_key = storage.get_secret_key(&secret_key.public_key(&secp)).unwrap();
        assert_eq!(priv_key, secret_key);
        std::fs::remove_dir_all(path).unwrap();
    }
}

// /// Error type for [`Store::aggregate_changesets`].
// #[derive(Debug)]
// pub struct AggregateChangesetsError<C> {
//     /// The partially-aggregated changeset.
//     pub changeset: Option<C>,
//
//     /// The error returned by the iterator.
//     pub iter_error: IterError,
// }
//
// impl<C> std::fmt::Display for AggregateChangesetsError<C> {
//     fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
//         std::fmt::Display::fmt(&self.iter_error, f)
//     }
// }
//
// impl<C: std::fmt::Debug> std::error::Error for AggregateChangesetsError<C> {}
//
// // You'll need to update the IterError to include Sled errors:
// #[derive(Debug)]
// pub enum IterError {
//     Bincode(Box<bincode::ErrorKind>),
//     Sled(sled::Error),
// }
//
// impl std::fmt::Display for IterError {
//     fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
//         match self {
//             IterError::Bincode(e) => write!(f, "Bincode error: {}", e),
//             IterError::Sled(e) => write!(f, "Sled error: {}", e),
//         }
//     }
// }
//
// impl std::error::Error for IterError {}

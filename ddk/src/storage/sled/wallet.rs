use super::SledStorageProvider;
use crate::error::WalletError;
use crate::signer::{DeriveSigner, SignerInformation};
use bdk_chain::Merge;
use bdk_wallet::ChangeSet;
use bdk_wallet::WalletPersister;
use bitcoin::secp256k1::{PublicKey, SecretKey};

const CHANGESET_KEY: &str = "changeset";

impl WalletPersister for SledStorageProvider {
    type Error = WalletError;

    fn persist(persister: &mut Self, changeset: &ChangeSet) -> Result<(), Self::Error> {
        tracing::info!("Presisting changeset to wallet persistance.");
        let wallet_tree = persister.wallet_tree()?;
        let new_changeset = if let Some(cs) = wallet_tree.get(CHANGESET_KEY)? {
            let mut stored_changeset = bincode::deserialize::<ChangeSet>(&cs)?;
            stored_changeset.merge(changeset.clone());
            stored_changeset
        } else {
            changeset.to_owned()
        };
        let new_changeset_bytes = bincode::serialize(&new_changeset)?;
        wallet_tree
            .insert(CHANGESET_KEY, new_changeset_bytes)
            .unwrap();
        Ok(())
    }

    fn initialize(persister: &mut Self) -> Result<ChangeSet, Self::Error> {
        tracing::info!("Initializing wallet persistance.");
        if let Some(cs) = persister.wallet_tree()?.get(CHANGESET_KEY)? {
            let cs = bincode::deserialize::<ChangeSet>(&cs)?;
            Ok(cs)
        } else {
            Ok(ChangeSet::default())
        }
    }
}

impl DeriveSigner for SledStorageProvider {
    type Error = WalletError;

    fn get_key_information(&self, key_id: [u8; 32]) -> Result<SignerInformation, Self::Error> {
        let key = hex::encode(key_id);
        let info = self.signer_tree()?.get(key)?.unwrap();
        Ok(bincode::deserialize::<SignerInformation>(&info)?)
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

        // Store the key id string instead of bytes.
        let key_id = hex::encode(key_id);

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

        Err(WalletError::SignerError(
            "Could not find secret key.".into(),
        ))
    }

    fn import_address_to_storage(&self, _address: &bitcoin::Address) -> Result<(), WalletError> {
        Ok(())
    }
}

//! p2pderivatives/rust-dlc <https://github.com/p2pderivatives/rust-dlc/blob/master/dlc-sled-storage-provider/src/lib.rs> (2024)
//! # dlc-sled-storage-provider
//! Storage provider for dlc-manager using sled as underlying storage.

mod contract;
mod wallet;

use crate::error::WalletError;
use crate::Storage;
use bdk_chain::Merge;
use bdk_wallet::ChangeSet;
use ddk_manager::contract::ser::Serializable;
use ddk_manager::error::Error;
use lightning::io::{Cursor, Read};
use sled::{Db, Tree};

const CONTRACT_TREE: u8 = 1;
const CHANNEL_TREE: u8 = 2;
pub const CHAIN_MONITOR_TREE: u8 = 3;
pub const CHAIN_MONITOR_KEY: u8 = 4;
const SIGNER_TREE: u8 = 6;
const WALLET_TREE: u8 = 7;
const MARKETPLACE_TREE: u8 = 8;
const CHANGESET_KEY: &str = "changeset";

/// Implementation of Storage interface using the sled DB backend.
#[derive(Debug, Clone)]
pub struct SledStorage {
    db: Db,
}

impl SledStorage {
    /// Creates a new instance of a SledStorage.
    pub fn new(path: &str) -> Result<Self, sled::Error> {
        Ok(SledStorage {
            db: sled::open(path)?,
        })
    }

    fn get_data_with_prefix<T: Serializable>(
        &self,
        tree: &Tree,
        prefix: &[u8],
        consume: Option<u64>,
    ) -> Result<Vec<T>, Error> {
        let iter = tree.iter();
        iter.values()
            .filter_map(|res| {
                let value = res.unwrap();
                let mut cursor = Cursor::new(&value);
                let mut pref = vec![0u8; prefix.len()];
                cursor.read_exact(&mut pref).expect("Error reading prefix");
                if pref == prefix {
                    if let Some(c) = consume {
                        cursor.set_position(cursor.position() + c);
                    }
                    Some(Ok(T::deserialize(&mut cursor).ok()?))
                } else {
                    None
                }
            })
            .collect()
    }

    fn open_tree(&self, tree_id: &[u8; 1]) -> Result<Tree, Error> {
        self.db
            .open_tree(tree_id)
            .map_err(|e| Error::StorageError(format!("Error opening contract tree: {}", e)))
    }

    fn contract_tree(&self) -> Result<Tree, Error> {
        self.open_tree(&[CONTRACT_TREE])
    }

    fn channel_tree(&self) -> Result<Tree, Error> {
        self.open_tree(&[CHANNEL_TREE])
    }

    fn signer_tree(&self) -> Result<Tree, sled::Error> {
        self.db.open_tree([SIGNER_TREE])
    }

    pub fn wallet_tree(&self) -> Result<Tree, sled::Error> {
        self.db.open_tree([WALLET_TREE])
    }

    pub fn marketplace_tree(&self) -> Result<Tree, sled::Error> {
        self.db.open_tree([MARKETPLACE_TREE])
    }
}

#[async_trait::async_trait]
impl Storage for SledStorage {
    async fn persist_bdk(&self, changeset: &ChangeSet) -> Result<(), WalletError> {
        let wallet_tree = self.wallet_tree().map_err(sled_to_wallet_error)?;
        let new_changeset = match wallet_tree
            .get(CHANGESET_KEY)
            .map_err(sled_to_wallet_error)?
        {
            Some(stored_changeset) => {
                let mut stored_changeset = serde_json::from_slice::<ChangeSet>(&stored_changeset)?;
                stored_changeset.merge(changeset.clone());
                stored_changeset
            }
            None => changeset.to_owned(),
        };

        wallet_tree
            .insert(CHANGESET_KEY, serde_json::to_vec(&new_changeset)?)
            .map_err(sled_to_wallet_error)?;
        Ok(())
    }

    async fn initialize_bdk(&self) -> Result<ChangeSet, WalletError> {
        tracing::info!("Initializing wallet persistance.");
        let changeset = match self
            .wallet_tree()
            .map_err(sled_to_wallet_error)?
            .get(CHANGESET_KEY)
            .map_err(sled_to_wallet_error)?
        {
            Some(changeset) => serde_json::from_slice(&changeset)?,
            None => ChangeSet::default(),
        };
        Ok(changeset)
    }
}

fn sled_to_wallet_error(error: sled::Error) -> WalletError {
    WalletError::StorageError(error.to_string())
}

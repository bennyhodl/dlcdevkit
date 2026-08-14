//! p2pderivatives/rust-dlc <https://github.com/p2pderivatives/rust-dlc/blob/master/dlc-sled-storage-provider/src/lib.rs> (2024)
//! # dlc-sled-storage-provider
//! Storage provider for dlc-manager using sled as underlying storage.

mod contract;
mod wallet;

use std::sync::Arc;

use crate::logger::{log_info, WriteLog};
use crate::Storage;
use crate::{error::WalletError, logger::Logger};
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
const LABEL_TREE: u8 = 9;
const CHANGESET_KEY: &str = "changeset";
const CONTRACT_TRACKER_KEY: &str = "contract_tracker";

/// Implementation of Storage interface using the sled DB backend.
#[derive(Debug, Clone)]
pub struct SledStorage {
    db: Db,
    logger: Arc<Logger>,
}

impl SledStorage {
    /// Creates a new instance of a SledStorage.
    pub fn new(path: &str, logger: Arc<Logger>) -> Result<Self, sled::Error> {
        Ok(SledStorage {
            db: sled::open(path)?,
            logger,
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

    fn label_tree(&self) -> Result<Tree, sled::Error> {
        self.db.open_tree([LABEL_TREE])
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
        log_info!(self.logger, "Initializing sled wallet persistance.");
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

    async fn initialize_contract_tracker(
        &self,
    ) -> Result<crate::wallet::contract_tracker::ChangeSet, WalletError> {
        let changeset = match self
            .wallet_tree()
            .map_err(sled_to_wallet_error)?
            .get(CONTRACT_TRACKER_KEY)
            .map_err(sled_to_wallet_error)?
        {
            Some(changeset) => serde_json::from_slice(&changeset)?,
            None => crate::wallet::contract_tracker::ChangeSet::default(),
        };
        Ok(changeset)
    }

    async fn persist_contract_tracker(
        &self,
        changeset: &crate::wallet::contract_tracker::ChangeSet,
    ) -> Result<(), WalletError> {
        let wallet_tree = self.wallet_tree().map_err(sled_to_wallet_error)?;
        let new_changeset = match wallet_tree
            .get(CONTRACT_TRACKER_KEY)
            .map_err(sled_to_wallet_error)?
        {
            Some(stored) => {
                let mut stored =
                    serde_json::from_slice::<crate::wallet::contract_tracker::ChangeSet>(&stored)?;
                stored.merge(changeset.clone());
                stored
            }
            None => changeset.to_owned(),
        };

        wallet_tree
            .insert(CONTRACT_TRACKER_KEY, serde_json::to_vec(&new_changeset)?)
            .map_err(sled_to_wallet_error)?;
        Ok(())
    }

    async fn load_labels(&self) -> Result<bip329::Labels, WalletError> {
        let labels = self
            .label_tree()
            .map_err(sled_to_wallet_error)?
            .iter()
            .values()
            .map(|value| {
                let value = value.map_err(sled_to_wallet_error)?;
                Ok(serde_json::from_slice::<bip329::Label>(&value)?)
            })
            .collect::<Result<Vec<_>, WalletError>>()?;
        Ok(bip329::Labels::new(labels))
    }

    async fn persist_label(&self, label: &bip329::Label) -> Result<(), WalletError> {
        self.label_tree()
            .map_err(sled_to_wallet_error)?
            .insert(
                crate::storage::label_key(&label.ref_()).as_bytes(),
                serde_json::to_vec(label)?,
            )
            .map_err(sled_to_wallet_error)?;
        Ok(())
    }

    async fn delete_label(&self, label_ref: &bip329::LabelRef) -> Result<(), WalletError> {
        self.label_tree()
            .map_err(sled_to_wallet_error)?
            .remove(crate::storage::label_key(label_ref).as_bytes())
            .map_err(sled_to_wallet_error)?;
        Ok(())
    }
}

fn sled_to_wallet_error(error: sled::Error) -> WalletError {
    WalletError::StorageError(error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use bitcoin::hashes::Hash;
    use bitcoin::{OutPoint, Txid};

    #[tokio::test]
    async fn labels_round_trip() {
        let path = "tests/data/dlc_storagedb/labels_round_trip";
        let logger = Arc::new(Logger::disabled("sled_test".to_string()));
        {
            let storage = SledStorage::new(path, logger).expect("Error opening sled DB");
            let txid = Txid::from_byte_array([0xAB; 32]);

            let label = bip329::Label::Transaction(bip329::TransactionRecord {
                ref_: txid,
                label: Some("DLC funding".to_string()),
                origin: None,
            });
            storage.persist_label(&label).await.unwrap();

            let output_label = bip329::Label::Output(bip329::OutputRecord {
                ref_: OutPoint { txid, vout: 0 },
                label: Some("collateral".to_string()),
                spendable: Some(false),
            });
            storage.persist_label(&output_label).await.unwrap();

            let labels = storage.load_labels().await.unwrap();
            assert_eq!(labels.iter().count(), 2);

            storage.delete_label(&label.ref_()).await.unwrap();
            let labels = storage.load_labels().await.unwrap();
            assert_eq!(labels.iter().count(), 1);
        }
        std::fs::remove_dir_all(path).unwrap();
    }

    #[tokio::test]
    async fn contract_tracker_round_trips() {
        use crate::wallet::contract_tracker;

        let path = "tests/data/dlc_storagedb/contract_tracker_round_trips";
        let logger = Arc::new(Logger::disabled("sled_test".to_string()));
        {
            let storage = SledStorage::new(path, logger).expect("Error opening sled DB");

            let spk =
                bitcoin::ScriptBuf::new_p2wsh(&bitcoin::WScriptHash::from_byte_array([0xCD; 32]));
            let mut changeset = contract_tracker::ChangeSet::default();
            changeset.spks.insert([0x5A; 32], spk.clone());
            changeset
                .tx_graph
                .last_seen
                .insert(Txid::from_byte_array([0xAB; 32]), 100);
            storage.persist_contract_tracker(&changeset).await.unwrap();
            let read = storage.initialize_contract_tracker().await.unwrap();
            assert_eq!(read, changeset);

            // A second persist merges instead of overwriting.
            let mut more = contract_tracker::ChangeSet::default();
            more.spks.insert([0x5B; 32], spk);
            storage.persist_contract_tracker(&more).await.unwrap();
            let read = storage.initialize_contract_tracker().await.unwrap();
            assert_eq!(read.spks.len(), 2);
            assert_eq!(read.tx_graph.last_seen.len(), 1);
        }
        std::fs::remove_dir_all(path).unwrap();
    }

    #[tokio::test]
    async fn locked_outpoints_round_trip() {
        let path = "tests/data/dlc_storagedb/locked_outpoints_round_trip";
        let logger = Arc::new(Logger::disabled("sled_test".to_string()));
        {
            let storage = SledStorage::new(path, logger).expect("Error opening sled DB");
            let outpoint = OutPoint {
                txid: Txid::from_byte_array([0xAB; 32]),
                vout: 1,
            };

            let mut lock = ChangeSet::default();
            lock.locked_outpoints.outpoints.insert(outpoint, true);
            storage.persist_bdk(&lock).await.unwrap();
            let read = storage.initialize_bdk().await.unwrap();
            assert_eq!(read.locked_outpoints.outpoints.get(&outpoint), Some(&true));

            // An unlock overwrites the lock.
            let mut unlock = ChangeSet::default();
            unlock.locked_outpoints.outpoints.insert(outpoint, false);
            storage.persist_bdk(&unlock).await.unwrap();
            let read = storage.initialize_bdk().await.unwrap();
            assert_eq!(read.locked_outpoints.outpoints.get(&outpoint), Some(&false));
        }
        std::fs::remove_dir_all(path).unwrap();
    }
}

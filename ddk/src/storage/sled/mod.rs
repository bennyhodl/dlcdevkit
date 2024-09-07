//! p2pderivatives/rust-dlc https://github.com/p2pderivatives/rust-dlc/blob/master/dlc-sled-storage-provider/src/lib.rs (2024)
//! # dlc-sled-storage-provider
//! Storage provider for dlc-manager using sled as underlying storage.

mod contract;
mod wallet;

use dlc_manager::contract::ser::Serializable;
use dlc_manager::error::Error;
use sled::{Db, Tree};
use lightning::io::{Cursor, Read};

use crate::transport::PeerInformation;
use crate::DdkStorage;

const CONTRACT_TREE: u8 = 1;
const CHANNEL_TREE: u8 = 2;
pub const CHAIN_MONITOR_TREE: u8 = 3;
pub const CHAIN_MONITOR_KEY: u8 = 4;
const PEER_KEY: u8 = 5;
const SIGNER_TREE: u8 = 6;
const WALLET_TREE: u8 = 7;

/// Implementation of Storage interface using the sled DB backend.
#[derive(Debug, Clone)]
pub struct SledStorageProvider {
    db: Db,
}

impl SledStorageProvider {
    /// Creates a new instance of a SledStorageProvider.
    pub fn new(path: &str) -> Result<Self, sled::Error> {
        Ok(SledStorageProvider {
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
        self.db.open_tree(&[SIGNER_TREE])
    }

    pub fn wallet_tree(&self) -> Result<Tree, sled::Error> {
        self.db.open_tree(&[WALLET_TREE])
    }
}

impl DdkStorage for SledStorageProvider {
    fn list_peers(&self) -> anyhow::Result<Vec<PeerInformation>> {
        if let Some(bytes) = self.db.get("peers")? {
            let peers: Vec<PeerInformation> = serde_json::from_slice(&bytes)?;
            Ok(peers)
        } else {
            Ok(vec![])
        }
    }

    fn save_peer(&self, peer: PeerInformation) -> anyhow::Result<()> {
        let mut known_peers = self.list_peers()?;

        if known_peers.contains(&peer) {
            return Ok(());
        }

        known_peers.push(peer);
        let peer_vec = serde_json::to_vec(&known_peers)?;

        self.db.insert("peers", peer_vec)?;

        Ok(())
    }
}

//! p2pderivatives/rust-dlc <https://github.com/p2pderivatives/rust-dlc/blob/master/dlc-sled-storage-provider/src/lib.rs> (2024)
//! # dlc-sled-storage-provider
//! Storage provider for dlc-manager using sled as underlying storage.

mod contract;
mod wallet;

use bdk_chain::Merge;
use bdk_wallet::ChangeSet;
use dlc_manager::contract::ser::Serializable;
use dlc_manager::error::Error;
use dlc_messages::oracle_msgs::OracleAnnouncement;
use lightning::io::{Cursor, Read};
use sled::{Db, Tree};

use crate::error::WalletError;
use crate::transport::PeerInformation;
use crate::Storage;

const CONTRACT_TREE: u8 = 1;
const CHANNEL_TREE: u8 = 2;
pub const CHAIN_MONITOR_TREE: u8 = 3;
pub const CHAIN_MONITOR_KEY: u8 = 4;
const SIGNER_TREE: u8 = 6;
const WALLET_TREE: u8 = 7;
const MARKETPLACE_TREE: u8 = 8;

const MARKETPLACE_KEY: &str = "marketplace";
const CHANGESET_KEY: &str = "changeset";
const PEERS_KEY: &str = "peers";

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
        self.db.open_tree(&[SIGNER_TREE])
    }

    pub fn wallet_tree(&self) -> Result<Tree, sled::Error> {
        self.db.open_tree(&[WALLET_TREE])
    }

    pub fn marketplace_tree(&self) -> Result<Tree, sled::Error> {
        self.db.open_tree(&[MARKETPLACE_TREE])
    }
}

impl Storage for SledStorage {
    fn persist_bdk(&self, changeset: &ChangeSet) -> Result<(), WalletError> {
        let wallet_tree = self.wallet_tree()?;
        let new_changeset = match wallet_tree.get(CHANGESET_KEY)? {
            Some(stored_changeset) => {
                let mut stored_changeset = bincode::deserialize::<ChangeSet>(&stored_changeset)?;
                stored_changeset.merge(changeset.clone());
                stored_changeset
            }
            None => changeset.to_owned(),
        };

        wallet_tree.insert(CHANGESET_KEY, bincode::serialize(&new_changeset)?)?;
        Ok(())
    }

    fn initialize_bdk(&self) -> Result<ChangeSet, WalletError> {
        tracing::info!("Initializing wallet persistance.");
        let changeset = match self.wallet_tree()?.get(CHANGESET_KEY)? {
            Some(changeset) => bincode::deserialize::<ChangeSet>(&changeset)?,
            None => ChangeSet::default(),
        };
        Ok(changeset)
    }

    fn list_peers(&self) -> anyhow::Result<Vec<PeerInformation>> {
        if let Some(bytes) = self.db.get(PEERS_KEY)? {
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

    fn save_announcement(&self, announcement: OracleAnnouncement) -> anyhow::Result<()> {
        let marketplace = self.marketplace_tree()?;
        let stored_announcements: Vec<OracleAnnouncement> =
            match marketplace.get(MARKETPLACE_KEY)? {
                Some(o) => bincode::deserialize(&o)?,
                None => vec![],
            };
        let mut announcements =
            crate::util::filter_expired_oracle_announcements(stored_announcements);
        announcements.push(announcement);

        let serialize_announcements = bincode::serialize(&announcements)?;
        marketplace.insert(MARKETPLACE_KEY, serialize_announcements)?;

        Ok(())
    }

    fn get_marketplace_announcements(&self) -> anyhow::Result<Vec<OracleAnnouncement>> {
        let marketplace = self.marketplace_tree()?;
        let prev_announcements = match marketplace.get(MARKETPLACE_KEY)? {
            Some(o) => o.to_vec(),
            None => vec![],
        };
        let announcements: Vec<OracleAnnouncement> = bincode::deserialize(&prev_announcements)?;
        Ok(announcements)
    }
}

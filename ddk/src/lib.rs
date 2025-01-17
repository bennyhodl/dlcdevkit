//! application tooling for DLCs 🌊
// #![doc = include_str!("../README.md")]
#![allow(dead_code)]
// #![allow(unused_imports)]
/// Build a DDK application.
pub mod builder;
/// Working with the bitcoin chain.
pub mod chain;
mod ddk;
/// DDK error types
pub mod error;
/// JSON structs
pub mod json;
/// Nostr related functions.
#[cfg(any(feature = "nostr", feature = "marketplace"))]
pub(crate) mod nostr;
/// Oracle clients.
pub mod oracle;
/// Storage implementations.
pub mod storage;
/// Transport services.
pub mod transport;
/// DLC utilities.
pub mod util;
/// The internal [`bdk_wallet::PersistedWallet`].
pub mod wallet;

/// DDK object with all services
pub use ddk::DlcDevKit;
pub use ddk::DlcManagerMessage;
pub use ddk_manager;

/// Default nostr relay.
pub const DEFAULT_NOSTR_RELAY: &str = "wss://nostr.dlcdevkit.com";

use async_trait::async_trait;
use bdk_wallet::ChangeSet;
use bitcoin::secp256k1::{PublicKey, SecretKey};
use bitcoin::Amount;
use ddk::DlcDevKitDlcManager;
use dlc_messages::oracle_msgs::OracleAnnouncement;
use dlc_messages::Message;
use error::WalletError;
use std::sync::Arc;
use tokio::sync::watch;
use transport::PeerInformation;

#[async_trait]
/// Allows ddk to open a listening connection and send/receive dlc messages functionality.
pub trait Transport: Send + Sync + 'static {
    /// Name for the transport service.
    fn name(&self) -> String;
    /// Get the public key of the transport.
    fn public_key(&self) -> PublicKey;
    /// Get messages that have not been processed yet.
    async fn start<S: Storage, O: Oracle>(
        &self,
        mut stop_signal: watch::Receiver<bool>,
        manager: Arc<DlcDevKitDlcManager<S, O>>,
    ) -> Result<(), anyhow::Error>;
    /// Send a message to a specific counterparty.
    async fn send_message(&self, counterparty: PublicKey, message: Message);
    /// Connect to another peer
    async fn connect_outbound(&self, pubkey: PublicKey, host: &str);
}

/// Storage for DLC contracts.
pub trait Storage: ddk_manager::Storage + Send + Sync + 'static {
    ///// Instantiate the storage for the BDK wallet.
    fn initialize_bdk(&self) -> Result<ChangeSet, WalletError>;
    /// Save changeset to the wallet storage.
    fn persist_bdk(&self, changeset: &ChangeSet) -> Result<(), WalletError>;
    /// Connected counterparties.
    fn list_peers(&self) -> anyhow::Result<Vec<PeerInformation>>;
    /// Persis counterparty.
    fn save_peer(&self, peer: PeerInformation) -> anyhow::Result<()>;
    // #[cfg(feature = "marketplace")]
    fn save_announcement(&self, announcement: OracleAnnouncement) -> anyhow::Result<()>;
    // #[cfg(feature = "marketplace")]
    fn get_marketplace_announcements(&self) -> anyhow::Result<Vec<OracleAnnouncement>>;
}

/// Retrieval of key material for signing DLC transactions
pub trait KeyStorage {
    fn get_secret_key(&self, key_id: [u8; 32]) -> Result<SecretKey, WalletError>;
    fn store_secret_key(&self, key_id: [u8; 32], secret_key: SecretKey) -> Result<(), WalletError>;
}

/// Oracle client
pub trait Oracle: ddk_manager::Oracle + Send + Sync + 'static {
    fn name(&self) -> String;
}

#[derive(Debug, Clone, Copy, serde::Serialize, serde::Deserialize)]
pub struct Balance {
    /// Confirmed in the wallet.
    pub confirmed: Amount,
    /// Unconfirmed UTXO that is owned by the wallet. Typically change.
    pub change_unconfirmed: Amount,
    /// Unconfirmed UTXO not owned by the wallet.
    pub foreign_unconfirmed: Amount,
    /// UTXOs in an active contract.
    pub contract: Amount,
    /// Profit and loss in all closed contracts.
    pub contract_pnl: i64,
}

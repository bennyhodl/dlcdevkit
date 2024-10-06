//! application tooling for DLCs 🌊

#![allow(dead_code)]
// #![allow(unused_imports)]
mod chain;
mod ddk;
mod error;
mod signer;
#[cfg(test)]
mod test_util;

/// Build a DDK application.
pub mod builder;
/// IO utilities
pub mod io;
/// Nostr related functions.
pub mod nostr;
/// Oracle clients.
pub mod oracle;
/// Storage implementations.
pub mod storage;
/// Transport services.
pub mod transport;
/// DLC utilities.
pub mod util;
/// The internal [bdk::Wallet].
pub mod wallet;
/// DDK object with all services
pub use ddk::DlcDevKit;
/// Type alias for [dlc_manager::manager::Manager]
pub use ddk::DlcDevKitDlcManager;

pub use bdk_wallet::LocalOutput;
/// Re-exports
pub use bitcoin;
pub use dlc;
pub use dlc_manager;
pub use dlc_messages;

/// Nostr relay host. TODO: nostr feature
pub const RELAY_HOST: &str = "ws://localhost:8081";
/// Default, local oracle host.
pub const ORACLE_HOST: &str = "http://localhost:8080";
/// Default, local esplora host.
pub const ESPLORA_HOST: &str = "http://localhost:30000";

use async_trait::async_trait;
use bdk_wallet::WalletPersister;
use bitcoin::key::XOnlyPublicKey;
use bitcoin::secp256k1::PublicKey;
use dlc_messages::oracle_msgs::OracleAnnouncement;
use dlc_messages::Message;
use kormir::OracleAttestation;
use signer::DeriveSigner;
use transport::PeerInformation;

/// Allows ddk to open a listening connection and send/receive dlc messages functionality.
///
/// TODO: error handling and result types
#[async_trait]
pub trait Transport: std::marker::Send + std::marker::Sync + 'static {
    type PeerManager;
    type MessageHandler;

    /// Name for the transport service.
    fn name(&self) -> String;
    /// Open an incoming listener for DLC messages from peers.
    async fn listen(&self);
    /// Retrieve the message handler.
    /// TODO: could remove?
    fn message_handler(&self) -> Self::MessageHandler;
    /// Retrieve the peer handler.
    /// TODO: could remove?
    fn peer_manager(&self) -> Self::PeerManager;
    /// Process messages
    fn process_messages(&self);
    /// Send a message to a specific counterparty.
    fn send_message(&self, counterparty: PublicKey, message: Message);
    /// Get messages that have not been processed yet.
    fn get_and_clear_received_messages(&self) -> Vec<(PublicKey, Message)>;
    /// If their are messages that still need to be processed.
    fn has_pending_messages(&self) -> bool;
    /// Connect to another peer
    async fn connect_outbound(&self, pubkey: PublicKey, host: &str);
}

/// Storage for DLC contracts.
pub trait Storage:
    dlc_manager::Storage
    + DeriveSigner
    + std::marker::Send
    + std::marker::Sync
    + 'static
    + WalletPersister
{
    fn list_peers(&self) -> anyhow::Result<Vec<PeerInformation>>;
    fn save_peer(&self, peer: PeerInformation) -> anyhow::Result<()>;
    // #[cfg(feature = "marketplace")]
    fn save_announcement(&self, announcement: OracleAnnouncement) -> anyhow::Result<()>;
    // #[cfg(feature = "marketplace")]
    fn get_marketplace_announcements(&self) -> anyhow::Result<Vec<OracleAnnouncement>>;
}

/// Oracle client
#[async_trait]
pub trait Oracle: dlc_manager::Oracle + std::marker::Send + std::marker::Sync + 'static {
    fn name(&self) -> String;
    async fn get_announcement_async(
        &self,
        event_id: &str,
    ) -> Result<OracleAnnouncement, dlc_manager::error::Error>;
    async fn get_public_key_async(&self) -> Result<XOnlyPublicKey, dlc_manager::error::Error>;
    async fn get_attestation_async(
        &self,
        event_id: &str,
    ) -> Result<OracleAttestation, dlc_manager::error::Error>;
}

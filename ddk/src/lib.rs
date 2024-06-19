#![allow(dead_code)]
#![allow(unused_imports)]
mod chain;
mod config;
mod ddk;
mod error;
mod io;
mod signer;
mod wallet;

/// Build a DDK application.
pub mod builder;

/// Oracle available structs.
pub mod oracle;
/// Storage available structs.
pub mod storage;
/// Transport available structs.
pub mod transport;

/// Re-exports
pub use bdk;
pub use bitcoin::Network;
use bitcoin::secp256k1::PublicKey;
pub use config::{DdkConfig, SeedConfig};
pub use ddk::DlcDevKit;
pub use ddk::DlcDevKitDlcManager;
pub use dlc_manager;
pub use dlc_messages;

pub const RELAY_HOST: &str = "ws://localhost:8081";
pub const ORACLE_HOST: &str = "http://localhost:8080";
pub const ESPLORA_HOST: &str = "http://localhost:30000";

use async_trait::async_trait;
use dlc_messages::Message;

#[async_trait]
pub trait DdkTransport {
    type PeerManager;
    type MessageHandler;

    fn name(&self) -> String;
    async fn listen(&self);
    fn message_handler(&self) -> Self::MessageHandler;
    fn peer_manager(&self) -> Self::PeerManager;
    fn send_message(&self, counterparty: PublicKey, message: Message);
    fn get_and_clear_received_messages(&self) -> Vec<(PublicKey, Message)>;
    fn has_pending_messages(&self) -> bool;
}

pub trait DdkStorage: dlc_manager::Storage /*+ PersistBackend<ChangeSet> */ {}

pub trait DdkOracle: dlc_manager::Oracle {
    fn name(&self) -> String;
}

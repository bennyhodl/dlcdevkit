#![allow(dead_code)]
#![allow(unused_imports)]
mod chain;
mod ddk;
mod error;
mod io;
mod oracle;
mod signer;
mod wallet;
mod storage;

/// Build a DDK application.
pub mod builder;

/// Transport available structs.
mod transport;

use std::sync::Arc;

/// Re-exports
pub use bdk;
pub use bitcoin::Network;
pub use ddk::DlcDevKit;
pub use ddk::DlcDevKitDlcManager;
pub use dlc_manager;
pub use dlc_messages;
pub use io::get_dlc_dev_kit_dir;

pub const RELAY_HOST: &str = "ws://localhost:8081";
pub const ORACLE_HOST: &str = "http://localhost:8080";
pub const ESPLORA_HOST: &str = "http://localhost:30000";

use bdk::{chain::PersistBackend, wallet::ChangeSet};
use tokio::sync::Mutex;

#[async_trait::async_trait]
pub trait DdkTransport {
    async fn listen(&self); 
    async fn handle_dlc_message(&self, ddk: &Arc<Mutex<DlcDevKitDlcManager>>);
}

pub trait DdkStorage /*: dlc_manager::Storage + PersistBackend<ChangeSet> */ {}

pub trait DdkOracle /*: dlc_manager::Oracle */ {}

#![allow(dead_code)]
#![allow(unused_imports)]
mod chain;
mod ddk;
mod error;
mod io;
mod oracle;
mod signer;
mod wallet;

/// Build a DDK application.
pub mod builder;

/// Transport available structs.
mod transport;

/// Re-exports
pub use bdk;
pub use bitcoin::Network;
pub use ddk::DlcDevKit;
pub use dlc_manager;
pub use dlc_messages;
pub use dlc_sled_storage_provider::SledStorageProvider;
pub use io::get_dlc_dev_kit_dir;

pub const RELAY_HOST: &str = "ws://localhost:8081";
pub const ORACLE_HOST: &str = "http://localhost:8080";

use bdk::{chain::PersistBackend, wallet::ChangeSet};

pub trait DdkTransport {}

pub trait DdkStorage /*: dlc_manager::Storage + PersistBackend<ChangeSet> */ {}

pub trait DdkOracle /*: dlc_manager::Oracle */ {}

pub(crate) struct MockTransport {}

impl DdkTransport for MockTransport {}

pub(crate) struct MockStorage {}

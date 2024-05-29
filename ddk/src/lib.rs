#![allow(dead_code)]
#![allow(unused_imports)]
mod chain;
mod ddk;
mod error;
mod io;
mod oracle;
mod signer;
mod wallet;

pub mod builder;

/// Transport available structs.
mod transport;

/// Re-exports
pub use bdk;
pub use bitcoin::Network;
pub use dlc_manager;
pub use dlc_messages;
pub use dlc_sled_storage_provider::SledStorageProvider;
pub use io::get_dlc_dev_kit_dir;

pub const RELAY_HOST: &str = "ws://localhost:8081";
pub const ORACLE_HOST: &str = "http://localhost:8080";

pub trait DdkTransport {}

pub trait DdkStorage {}

pub trait DdkOracle: dlc_manager::Oracle {}

struct MockTransport {}

impl DdkTransport for MockTransport {}

struct MockStorage {}

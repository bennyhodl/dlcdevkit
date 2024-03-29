#![allow(dead_code)]
#![allow(unused_imports)]
mod error;
mod io;
mod oracle;
mod wallet;
mod chain;

pub mod nostr_manager;
pub mod p2p;
pub use bitcoin::Network;
pub use dlc_manager;
pub use dlc_messages;
pub use dlc_sled_storage_provider::SledStorageProvider;
pub use io::get_ernest_dir;

pub const RELAY_HOST: &str = "ws://localhost:8081";
pub const ORACLE_HOST: &str = "http://localhost:8080";

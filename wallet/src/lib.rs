#![allow(dead_code)]
#![allow(unused_imports)]
// mod dlc_storage;
mod error;
mod io;
pub mod nostr_manager;
mod oracle;
pub mod peer_manager;
mod wallet;

pub use bitcoin::Network;
pub use io::get_ernest_dir;
pub use dlc_sled_storage_provider::SledStorageProvider;

pub const RELAY_HOST: &str = "ws://localhost:8081";
pub const ORACLE_HOST: &str = "http://localhost:8080";

#![allow(dead_code)]
#![allow(unused_imports)]
mod dlc_storage;
mod error;
mod io;
mod nostr;
mod oracle;
mod peer_manager;
mod wallet;

pub use bitcoin::Network;
pub use dlc_storage::SledStorageProvider;
pub use io::get_ernest_dir;
pub use nostr::ErnestNostr;
pub use nostr::{dlc_handler::NostrDlcHandler, relay_handler::NostrDlcRelayHandler};
pub use peer_manager::Ernest;
pub use peer_manager::peer_manager::ErnestPeerManager;

pub const RELAY_URL: &str = "ws://localhost:8081";

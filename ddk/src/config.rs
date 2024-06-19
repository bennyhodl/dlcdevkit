use std::path::PathBuf;

use bitcoin::Network;

pub const DEFAULT_STORAGE_DIR: &str = "/tmp/ddk";

/// I think this should be a requirement for all implementations in some way.
/// Need to have the directory created. Maybe instead when config is set, create the dir?
/// As well some might rely on seed. Ex. transport with nostr & ln.
#[derive(Debug, Clone)]
pub struct DdkConfig {
    pub network: Network,
    pub esplora_host: String,
    /// Probably an enum? Or is this even used? Maybe wallet_storage_path?
    pub storage_path: PathBuf,
    pub seed_config: SeedConfig,
}

impl Default for DdkConfig {
    fn default() -> Self {
        Self {
            network: Network::Signet,
            esplora_host: "https://mutinynet.com/api".to_string(),
            storage_path: DEFAULT_STORAGE_DIR.into(),
            seed_config: SeedConfig::default(),
        }
    }
}

#[derive(Debug, Clone)]
pub enum SeedConfig {
    Bytes([u8; 64]),
    File(String),
}

impl Default for SeedConfig {
    fn default() -> Self {
        Self::Bytes([0u8; 64])
    }
}

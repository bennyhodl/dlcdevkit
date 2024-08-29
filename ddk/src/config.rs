use std::{fmt, path::PathBuf};

use bitcoin::Network;

pub const DEFAULT_STORAGE_DIR: &str = "/tmp/ddk";

/// Configuration values for creating a DDK process.
///
/// I think this should be a requirement for all implementations in some way.
/// Need to have the directory created. Maybe instead when config is set, create the dir?
/// As well some might rely on seed. Ex. transport with nostr & ln.
#[derive(Debug, Clone)]
pub struct DdkConfig {
    /// The bitcoin network to run on. Defaults to mutiny net.
    pub network: Network,
    /// The esplora API to call to. Defaults to mutiny net
    pub esplora_host: String,
    /// The directory the DDK instance will be stored at. Defaults to /tmp/ddk/.
    /// Probably an enum? Or is this even used? Maybe wallet_storage_path?
    /// TODO: no-std config
    pub storage_path: PathBuf,
    /// The seed bytes, file, or mnemonic services will use. Defaults to [0u8; 64].
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

/// Seed configuration for DDK.
#[derive(Debug, Clone)]
pub enum SeedConfig {
    /// Seed bytes
    Bytes([u8; 64]),
    /// File path to a seed.
    File(String),
}

impl fmt::Display for SeedConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::File(_) => write!(f, "File"),
            Self::Bytes(_) => write!(f, "Bytes")
        }
    }
}

impl Default for SeedConfig {
    fn default() -> Self {
        Self::Bytes([0u8; 64])
    }
}

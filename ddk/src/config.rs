use serde::{Serialize, Deserialize};

#[derive(Debug, Clone, Default)]
pub struct DdkConfig {
    /// Probably an enum? Or is this even used? Maybe wallet_storage_path?
    pub storage_path: String,
    pub seed: SeedConfig,
}

#[derive(Debug, Clone)]
pub enum SeedConfig {
    Bytes([u8;64]),
    File(String),
}

impl Default for SeedConfig {
    fn default() -> Self {
        Self::Bytes([0u8; 64])
    }
}

impl DdkConfig {
    pub(crate) fn seed(&self) -> SeedConfig {
        self.seed.clone()
    }
}

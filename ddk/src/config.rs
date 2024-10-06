use std::fmt;

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
            Self::File(_) => write!(f, "file"),
            Self::Bytes(_) => write!(f, "bytes"),
        }
    }
}

impl Default for SeedConfig {
    fn default() -> Self {
        Self::Bytes([0u8; 64])
    }
}

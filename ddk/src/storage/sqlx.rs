use serde::{Deserialize, Serialize};
use sqlx::FromRow;

/// Crate error
#[derive(Debug, thiserror::Error)]
pub enum SqlxError {
    /// bitcoin parse hex error
    #[error("bitoin parse hex error: {0}")]
    HexToArray(#[from] bitcoin::hex::HexToArrayError),
    /// serde_json error
    #[error("serde_json error: {0}")]
    SerdeJson(#[from] serde_json::error::Error),
    /// sqlx error
    #[error("sqlx error: {0}")]
    Sqlx(#[from] sqlx::Error),
    /// migrate error
    #[error("migrate error: {0}")]
    Migrate(#[from] sqlx::migrate::MigrateError),
    /// serialize contract error
    #[error("serialize contract error: {0}")]
    SerializeContract(#[from] bitcoin::io::Error),
    #[error("deserialize contract error: {0}")]
    DeserializeContract(#[from] ddk_manager::error::Error),
    /// miniscript error
    #[error("miniscript error: {0}")]
    Miniscript(#[from] bdk_chain::miniscript::Error),
    #[error("Custom error: {0}")]
    Custom(String),
}

#[derive(Debug, Clone, FromRow, Serialize, Deserialize)]
pub struct ContractMetadata {
    pub id: String,
    pub state: i16,
    pub is_offer_party: bool,
    pub counter_party: String,
    pub offer_collateral: i64,
    pub total_collateral: i64,
    pub accept_collateral: i64,
    pub fee_rate_per_vb: i64,
    pub cet_locktime: i32,
    pub refund_locktime: i32,
    pub pnl: Option<i64>,
    pub funding_txid: Option<String>,
    pub cet_txid: Option<String>,
    pub announcement_id: String,
    pub oracle_pubkey: String,
}

#[derive(Debug, Clone, FromRow, Serialize, Deserialize)]
pub struct ContractData {
    pub id: String,
    pub contract_data: Vec<u8>,
    pub is_compressed: bool,
}

#[derive(Debug, Clone, FromRow, Serialize, Deserialize)]
pub struct ContractMetadataRow {
    pub id: String,
    pub state: i16,
    pub is_offer_party: bool,
    pub counter_party: String,
    pub offer_collateral: i64,
    pub accept_collateral: i64,
    pub total_collateral: i64,
    pub fee_rate_per_vb: i64,
    pub cet_locktime: i32,
    pub refund_locktime: i32,
    pub pnl: Option<i64>,
}

#[derive(Debug, Clone, FromRow, Serialize, Deserialize)]
pub struct ContractDataRow {
    pub id: String,
    pub contract_data: Vec<u8>,
    pub is_compressed: bool,
}

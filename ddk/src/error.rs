use bdk_esplora::esplora_client::Error as EsploraError;
use ddk_manager::error::Error as ManagerError;

pub fn esplora_err_to_manager_err(e: EsploraError) -> ManagerError {
    ManagerError::BlockchainError(e.to_string())
}

pub fn wallet_err_to_manager_err(e: WalletError) -> ManagerError {
    ManagerError::WalletError(Box::new(e))
}

/// BDK and wallet storage errors
#[derive(thiserror::Error, Debug)]
pub enum WalletError {
    #[error("Wallet Persistance error.")]
    WalletPersistanceError,
    #[error("Seed error: {0}")]
    Seed(#[from] bitcoin::bip32::Error),
    #[error("Failed to get lock on BDK wallet.")]
    Lock,
    #[error("Error syncing the internal BDK wallet.")]
    SyncError,
    #[error("Storage error. {0}")]
    StorageError(String),
    #[error("Error with deriving signer: {0}")]
    SignerError(String),
    #[error("Wallet call to esplora: {0}")]
    Esplora(#[from] Box<bdk_esplora::esplora_client::Error>),
    #[error("Broadcast to esplora: {0}")]
    Broadcast(#[from] bdk_esplora::esplora_client::Error),
    #[error("Could not extract txn from psbt. {0}")]
    ExtractTx(#[from] bitcoin::psbt::ExtractTxError),
    #[error("Applying an update to the wallet.")]
    UtxoUpdate(#[from] bdk_chain::local_chain::CannotConnectError),
    #[error("Error signing PSBT: {0}")]
    Signing(#[from] bdk_wallet::signer::SignerError),
    #[error("Receive error from wallet channel: {0}")]
    ReceiveMessage(#[from] crossbeam::channel::RecvError),
    #[error("Sending error from wallet channel: {0}")]
    SendMessage(String),
    #[error("Serde error: {0}")]
    Serde(#[from] serde_json::Error),
    #[error("Error converting to descriptor.")]
    Descriptor(#[from] bdk_wallet::descriptor::DescriptorError),
}

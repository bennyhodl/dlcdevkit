use bdk_esplora::esplora_client::Error as EsploraError;
use ddk_manager::error::Error as DlcManagerError;
use thiserror::Error;

#[derive(Error, Debug)]
pub enum Error {
    #[error("DDK runtime has already been initialized.")]
    RuntimeExists,
    #[error("DDK is not runnging.")]
    NoRuntime,
    #[error("WalletError: {0}")]
    Wallet(#[from] WalletError),
    #[error("TransportError: {0}")]
    Transport(#[from] TransportError),
    #[error("OracleError: {0}")]
    Oracle(#[from] OracleError),
    #[error("StorageError: {0}")]
    Storage(#[from] StorageError),
    #[error("ActorSendError: {0}")]
    ActorSendError(String),
    #[error("ActorReceiveError: {0}")]
    ActorReceiveError(String),
    #[error("ManagerError: {0}")]
    Manager(#[from] DlcManagerError),
    #[error("BuilderError: {0}")]
    Builder(#[from] BuilderError),
    #[error("EsploraError: {0}")]
    Esplora(#[from] bdk_esplora::esplora_client::Error),
    #[error("Generic error: {0}")]
    Generic(String),
    #[cfg(feature = "nostr")]
    #[error("NostrError: {0}")]
    Nostr(#[from] NostrError),
}

#[derive(Error, Debug)]
pub enum StorageError {
    #[error("Storage initialization: {0}")]
    Init(String),
    #[error("Sqlx storage error: {0}")]
    #[cfg(feature = "postgres")]
    Sqlx(#[from] crate::storage::sqlx::SqlxError),
}

#[derive(Error, Debug)]
pub enum OracleError {
    #[error("Oracle initialization: {0}")]
    Init(String),
    #[error("Oracle announcement error: {0}")]
    Announcement(String),
    #[error("Oracle attestation error: {0}")]
    Attestation(String),
    #[error("Create oracle event error: {0}")]
    CreateEvent(String),
    #[error("Sign oracle event error: {0}")]
    SignEvent(String),
    #[error("Oracle error: {0}")]
    Custom(String),
    #[error("HTTP error: {0}")]
    #[cfg(any(feature = "p2pderivatives", feature = "kormir"))]
    Reqwest(#[from] reqwest::Error),
}

#[derive(Error, Debug)]
pub enum ManagerError {
    #[error("DlcManagerError: {0}")]
    DlcManager(#[from] DlcManagerError),
}

#[derive(Error, Debug)]
pub enum TransportError {
    #[error("Transport initialization: {0}")]
    Init(String),
    #[error("Listen error: {0}")]
    Listen(String),
    #[error("Message processing error: {0}")]
    MessageProcessing(String),
}

#[derive(Error, Debug)]
pub enum NostrError {
    #[error("Nostr nip4: {0}")]
    Nip04(#[from] nostr_rs::nips::nip04::Error),
    #[error("Message parsing error: {0}")]
    MessageParsing(String),
    #[error("Signing nostr event error: {0}")]
    Signing(#[from] nostr_rs::event::builder::Error),
    #[error("Nostr generic: {0}")]
    Generic(String),
}

/// An error that could be thrown while building [`crate::ddk::DlcDevKit`]
#[derive(Debug, Clone, Copy, Error)]
pub enum BuilderError {
    #[error("A transport was not provided.")]
    NoTransport,
    #[error("A storage implementation was not provided.")]
    NoStorage,
    #[error("An oracle client was not provided.")]
    NoOracle,
}

/// BDK and wallet storage errors
#[derive(Error, Debug)]
pub enum WalletError {
    #[error("Wallet Persistance error: {0}")]
    WalletPersistanceError(String),
    #[error("Seed error: {0}")]
    Seed(#[from] bitcoin::bip32::Error),
    #[error("Failed to get lock on BDK wallet.")]
    Lock,
    #[error("Error syncing the internal BDK wallet.")]
    SyncError,
    #[error("Wallet Ssorage error. {0}")]
    StorageError(String),
    #[error("Signer error: {0}")]
    SignerError(String),
    #[error("TxnBuilder: Failed to build transaction. {0}")]
    TxnBuildeR(#[from] bdk_wallet::error::CreateTxError),
    #[error("Wallet call to esplora: {0}")]
    Esplora(String),
    #[error("Broadcast to esplora: {0}")]
    Broadcast(#[from] bdk_esplora::esplora_client::Error),
    #[error("Could not extract txn from psbt for sending.")]
    ExtractTx,
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

pub fn to_storage_error<T>(e: T) -> ddk_manager::error::Error
where
    T: std::fmt::Display,
{
    ddk_manager::error::Error::StorageError(e.to_string())
}

pub fn esplora_err_to_manager_err(e: EsploraError) -> DlcManagerError {
    DlcManagerError::BlockchainError(e.to_string())
}

pub fn wallet_err_to_manager_err(e: WalletError) -> DlcManagerError {
    DlcManagerError::WalletError(Box::new(e))
}

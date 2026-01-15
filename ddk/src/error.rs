use crate::wallet::WalletCommand;
use bdk_esplora::esplora_client::Error as EsploraError;
use ddk_manager::error::Error as DlcManagerError;
use thiserror::Error;

/// The main error type for the DDK library that consolidates all possible errors
/// that can occur throughout the project. This includes errors from:
/// - Runtime management
/// - Wallet operations
/// - Transport layer
/// - Oracle interactions
/// - Storage operations
/// - Actor communication
/// - DLC manager operations
/// - Builder process
/// - External services (Esplora)
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
    #[error("LoggerError: {0}")]
    Logger(#[from] LoggerError),
    #[error("EsploraError: {0}")]
    Esplora(#[from] bdk_esplora::esplora_client::Error),
    #[error("Generic error: {0}")]
    Generic(String),
    #[cfg(feature = "nostr")]
    #[error("NostrError: {0}")]
    Nostr(#[from] NostrError),
}

/// Errors related to storage operations in DDK.
/// Handles failures in:
/// - Storage initialization
/// - Database operations (when using PostgreSQL)
/// - Data persistence
#[derive(Error, Debug)]
pub enum StorageError {
    #[error("Storage initialization: {0}")]
    Init(String),
    #[error("Sqlx storage error: {0}")]
    #[cfg(feature = "postgres")]
    Sqlx(#[from] crate::storage::sqlx::SqlxError),
}

/// Errors related to oracle operations in DDK.
/// Handles failures in:
/// - Oracle initialization
/// - Event announcements and attestations
/// - Event creation and signing
/// - HTTP requests (for P2P derivatives and Kormir)
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

/// Errors from the DLC Manager component.
/// Currently wraps the underlying Rust-DLC manager errors.
#[derive(Error, Debug)]
pub enum ManagerError {
    #[error("DlcManagerError: {0}")]
    DlcManager(#[from] DlcManagerError),
}

/// Errors related to the transport layer in DDK.
/// Handles failures in:
/// - Transport initialization
/// - Connection listening
/// - Message processing and routing
#[derive(Error, Debug)]
pub enum TransportError {
    #[error("Transport initialization: {0}")]
    Init(String),
    #[error("Listen error: {0}")]
    Listen(String),
    #[error("Message processing error: {0}")]
    MessageProcessing(String),
}

/// Errors specific to Nostr protocol operations.
/// Only available when the "nostr" feature is enabled.
/// Handles failures in:
/// - NIP-04 encryption/decryption
/// - Message parsing
/// - Event signing
#[cfg(feature = "nostr")]
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

/// Errors related to logging operations in DDK.
/// Handles failures in:
/// - Logger initialization
/// - File creation and writing
/// - Log formatting
#[derive(Error, Debug)]
pub enum LoggerError {
    #[error("Logger initialization: {0}")]
    Init(String),
    #[error("File I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("Tracing setup error: {0}")]
    TracingSetup(String),
}

/// Errors that can occur during the DlcDevKit builder process.
/// These errors indicate missing required components when constructing
/// a new DlcDevKit instance.
#[derive(Debug, Clone, Copy, Error)]
pub enum BuilderError {
    #[error("A transport was not provided.")]
    NoTransport,
    #[error("A storage implementation was not provided.")]
    NoStorage,
    #[error("An oracle client was not provided.")]
    NoOracle,
    #[error("Failed to generate random seed.")]
    SeedGenerationFailed,
    #[error("Logger setup failed.")]
    LoggerSetupFailed,
}

/// Errors related to Bitcoin wallet operations.
/// Handles failures in:
/// - Wallet persistence
/// - Seed operations
/// - Transaction building and signing
/// - Chain synchronization
/// - UTXO management
/// - Communication with Esplora
/// - Actor message passing
#[derive(Error, Debug)]
pub enum WalletError {
    #[error("Wallet Persistance error: {0}")]
    WalletPersistanceError(String),
    #[error("Seed error: {0}")]
    Seed(#[from] bitcoin::bip32::Error),
    #[error("Error syncing the internal BDK wallet.")]
    SyncError,
    #[error("Wallet Ssorage error. {0}")]
    StorageError(String),
    #[error("Signer error: {0}")]
    SignerError(String),
    #[error("TxnBuilder: Failed to build transaction. {0}")]
    TxnBuilder(#[from] bdk_wallet::error::CreateTxError),
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
    #[error("Sending error from wallet channel: {0}")]
    SendMessage(String),
    #[error("Serde error: {0}")]
    Serde(#[from] serde_json::Error),
    #[error("Error converting to descriptor.")]
    Descriptor(#[from] bdk_wallet::descriptor::DescriptorError),
    #[error("Wallet receiver error: {0}")]
    Receiver(#[from] tokio::sync::oneshot::error::RecvError),
    #[error("Wallet sender error: {0}")]
    Sender(#[from] tokio::sync::mpsc::error::SendError<WalletCommand>),
    #[error("Invalid derivation index")]
    InvalidDerivationIndex,
    #[error("Invalid secret key")]
    InvalidSecretKey,
    #[error(
        "DESCRIPTOR MISMATCH DETECTED\n\n\
        {keychain} descriptor mismatch detected.\n\n\
        Expected descriptor:\n{expected}\n\n\
        Stored descriptor:\n{stored}\n\n\
        The wallet's stored descriptor doesn't match the descriptor\n\
        derived from the current seed. Please verify you're using the correct seed\n\
        or reset the wallet data if needed, but verify your wallet backups before resetting."
    )]
    DescriptorMismatch {
        keychain: String,
        expected: String,
        stored: String,
    },
}

/// Converts a generic error to a DLC manager storage error
pub fn to_storage_error<T>(e: T) -> ddk_manager::error::Error
where
    T: std::fmt::Display,
{
    ddk_manager::error::Error::StorageError(e.to_string())
}

/// Converts an Esplora error to a DLC manager error
pub fn esplora_err_to_manager_err(e: EsploraError) -> DlcManagerError {
    DlcManagerError::BlockchainError(e.to_string())
}

/// Converts a wallet error to a DLC manager error
pub fn wallet_err_to_manager_err(e: WalletError) -> DlcManagerError {
    DlcManagerError::WalletError(Box::new(e))
}

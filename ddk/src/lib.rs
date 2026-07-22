//! application tooling for DLCs 🌊
// #![doc = include_str!("../README.md")]
#![allow(clippy::result_large_err)]

/// Stateless DLC contract operations.
///
/// This is the only module available without the `manager` feature; it depends
/// on nothing heavier than `ddk-manager` and is what FFI/mobile consumers bind.
pub mod contract;

/// Build a DDK application.
#[cfg(feature = "manager")]
pub mod builder;
/// Working with the bitcoin chain.
#[cfg(feature = "manager")]
pub mod chain;
#[cfg(feature = "manager")]
mod ddk;
/// DDK error types
#[cfg(feature = "manager")]
pub mod error;
/// JSON structs
#[cfg(feature = "manager")]
pub mod json;
/// Logging infrastructure
#[cfg(feature = "manager")]
pub mod logger;
/// Nostr related functions.
#[cfg(feature = "nostr")]
pub mod nostr;
/// Oracle clients.
#[cfg(feature = "manager")]
pub mod oracle;
/// Storage implementations.
#[cfg(feature = "manager")]
pub mod storage;
/// Transport services.
#[cfg(feature = "manager")]
pub mod transport;
/// DLC utilities.
#[cfg(feature = "manager")]
pub mod util;
/// The internal [`bdk_wallet::PersistedWallet`].
#[cfg(feature = "manager")]
pub mod wallet;

pub use ddk_manager;

/// DDK object with all services
#[cfg(feature = "manager")]
pub use ddk::DlcDevKit;
#[cfg(feature = "manager")]
pub use ddk::DlcManagerMessage;

#[cfg(feature = "manager")]
use async_trait::async_trait;
#[cfg(feature = "manager")]
use bdk_wallet::ChangeSet;
#[cfg(feature = "manager")]
use bitcoin::secp256k1::{PublicKey, SecretKey};
#[cfg(feature = "manager")]
use bitcoin::Amount;
#[cfg(feature = "manager")]
use ddk::DlcDevKitDlcManager;
#[cfg(feature = "manager")]
use ddk_messages::Message;
#[cfg(feature = "manager")]
use error::TransportError;
#[cfg(feature = "manager")]
use error::WalletError;
#[cfg(feature = "manager")]
use std::sync::Arc;
#[cfg(feature = "manager")]
use tokio::sync::watch;

/// Transport layer for DLC message communication.
///
/// This trait defines the interface for sending and receiving DLC protocol messages
/// between peers. Implementations of this trait (found in the transport module) handle
/// the actual communication layer, such as:
/// - Lightning Network transport
/// - Nostr protocol messaging
/// - Direct TCP/IP connections
/// - In-memory transport (for testing)
///
/// # Implementation Requirements
/// - Must be Send + Sync for thread safety
/// - Must handle connection management
/// - Must support message serialization/deserialization
/// - Must maintain peer connections
///
/// # Usage
/// Implementations are used by the DLC manager to:
/// 1. Establish connections with counterparties
/// 2. Send protocol messages (offers, accepts, signs, etc.)
/// 3. Receive and process incoming messages
/// 4. Maintain connection state
#[async_trait]
#[cfg(feature = "manager")]
pub trait Transport: Send + Sync + 'static {
    /// Returns a unique identifier for this transport implementation.
    fn name(&self) -> String;

    /// Returns the transport's public key used for identification and message signing.
    fn public_key(&self) -> PublicKey;

    /// Starts the transport service and processes incoming messages.
    ///
    /// This method runs in its own task and handles:
    /// - Incoming message processing
    /// - Connection management
    /// - Message routing to the DLC manager
    /// - Graceful shutdown via stop signal
    ///
    /// # Arguments
    /// * `stop_signal` - Watch channel for graceful shutdown
    /// * `manager` - Reference to the DLC manager for message processing
    async fn start<S: Storage, O: Oracle>(
        &self,
        mut stop_signal: watch::Receiver<bool>,
        manager: Arc<DlcDevKitDlcManager<S, O>>,
    ) -> Result<(), TransportError>;

    /// Sends a DLC protocol message to a specific counterparty.
    ///
    /// # Arguments
    /// * `counterparty` - Public key of the message recipient
    /// * `message` - The DLC protocol message to send
    async fn send_message(&self, counterparty: PublicKey, message: Message);

    /// Establishes an outbound connection to a peer.
    ///
    /// # Arguments
    /// * `pubkey` - Public key of the peer to connect to
    /// * `host` - Network address of the peer
    async fn connect_outbound(&self, pubkey: PublicKey, host: &str);
}

/// Storage interface for DLC contracts and wallet data.
///
/// This trait extends the Rust-DLC storage trait (`ddk_manager::Storage`) with
/// additional functionality for BDK wallet integration. Implementations must be
/// thread-safe and handle interior mutability due to BDK's requirements.
///
/// # Implementation Notes
/// - Must be wrapped in synchronization primitives (Arc, Mutex, etc.)
/// - Must handle concurrent access to wallet data
/// - Should implement efficient caching where appropriate
/// - Must maintain consistency between DLC and wallet states
///
/// # Common Implementations
/// - PostgreSQL storage (persistent)
/// - Sled storage (persistent, embedded)
/// - In-memory storage (temporary, testing)
#[async_trait]
#[cfg(feature = "manager")]
pub trait Storage: ddk_manager::Storage + Send + Sync + std::fmt::Debug + 'static {
    /// Initializes the BDK wallet storage and returns initial state.
    ///
    /// This method is called during startup to:
    /// 1. Create necessary storage structures
    /// 2. Load existing wallet data
    /// 3. Initialize the wallet's change tracking
    async fn initialize_bdk(&self) -> Result<ChangeSet, WalletError>;

    /// Persists wallet changes to storage.
    ///
    /// This method handles:
    /// - Saving new transactions
    /// - Updating UTXO set
    /// - Maintaining wallet metadata
    async fn persist_bdk(&self, changeset: &ChangeSet) -> Result<(), WalletError>;
}

/// Interface for secure key material storage and retrieval.
///
/// NOTE: This trait is currently a placeholder for future key management functionality.
/// It will be expanded to handle more sophisticated key storage and derivation patterns.
///
/// # Future Enhancements
/// - Hardware wallet integration
/// - Key derivation paths
/// - Multi-signature support
/// - Key rotation policies
#[cfg(feature = "manager")]
pub trait KeyStorage {
    /// Retrieves a secret key by its identifier.
    fn get_secret_key(&self, key_id: [u8; 32]) -> Result<SecretKey, WalletError>;

    /// Stores a secret key with the given identifier.
    fn store_secret_key(&self, key_id: [u8; 32], secret_key: SecretKey) -> Result<(), WalletError>;
}

/// Interface for DLC oracle implementations.
///
/// This trait extends the Rust-DLC oracle trait (`ddk_manager::Oracle`) and provides
/// a way to identify different oracle implementations. Oracles are responsible for:
/// - Providing event announcements
/// - Publishing attestations
/// - Maintaining cryptographic proofs
///
/// # Common Implementations
/// - Nostr-based oracles
/// - API-based oracles
/// - Local testing oracles
#[cfg(feature = "manager")]
pub trait Oracle: ddk_manager::Oracle + Send + Sync + 'static {
    /// Returns the name of this oracle implementation.
    fn name(&self) -> String;
}

/// Represents the complete balance state of a DLC wallet.
///
/// This struct tracks various categories of funds in the wallet, including:
/// - Regular bitcoin balances (confirmed/unconfirmed)
/// - Funds locked in active DLC contracts
/// - Historical profit/loss from closed contracts
///
/// The separation of different balance types allows for:
/// - Accurate available balance calculation
/// - Contract fund tracking
/// - Performance monitoring
/// - Risk assessment
#[cfg(feature = "manager")]
#[derive(Debug, Clone, Copy, serde::Serialize, serde::Deserialize)]
pub struct Balance {
    /// Total confirmed balance in the wallet
    pub confirmed: Amount,

    /// Unconfirmed balance from change outputs
    pub change_unconfirmed: Amount,

    /// Unconfirmed balance from external sources
    pub foreign_unconfirmed: Amount,

    /// Total amount currently locked in active DLC contracts
    pub contract: Amount,

    /// Cumulative profit/loss from all closed contracts (in satoshis)
    /// Positive values indicate overall profit, negative values indicate loss
    pub contract_pnl: i64,
}

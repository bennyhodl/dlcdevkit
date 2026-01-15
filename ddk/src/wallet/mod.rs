//! # DDK Wallet Implementation
//!
//! This module provides the Bitcoin wallet functionality for DDK using BDK (Bitcoin Dev Kit)
//! with an actor-based architecture for thread-safe, lock-free operations.
//!
//! ## Storage Integration
//! The wallet uses a wrapper around DDK's Storage trait to provide BDK with the
//! AsyncWalletPersister interface. This ensures thread safety and interior mutability
//! requirements are met for BDK operations.
//!
//! ## Actor Model
//! The wallet implements an actor pattern using message passing to avoid locks and
//! ensure thread safety. All wallet operations are performed through commands sent
//! over tokio channels, allowing concurrent access from multiple components.
//!
//! ## Key Features
//! - Thread-safe wallet operations
//! - BDK integration for Bitcoin functionality
//! - Automatic chain synchronization
//! - PSBT signing for DLC operations
//! - Fee estimation
//! - UTXO management

pub mod address;
mod command;

use crate::error::{wallet_err_to_manager_err, WalletError};
use crate::logger::Logger;
use crate::logger::{log_error, log_info, WriteLog};
use crate::wallet::address::AddressGenerator;
use crate::{chain::EsploraClient, Storage};
use bdk_chain::Balance;
use bdk_wallet::coin_selection::{
    BranchAndBoundCoinSelection, CoinSelectionAlgorithm, SingleRandomDraw,
};
use bdk_wallet::descriptor::{Descriptor, IntoWalletDescriptor};
use bdk_wallet::keys::DescriptorPublicKey;
use bdk_wallet::AsyncWalletPersister;
pub use bdk_wallet::LocalOutput;
use bdk_wallet::{
    bitcoin::{
        bip32::Xpriv,
        secp256k1::{All, PublicKey, Secp256k1},
        Address, Network, Txid,
    },
    template::Bip84,
    AddressInfo, KeychainKind, SignOptions, Wallet,
};
use bdk_wallet::{Utxo, WeightedUtxo};
use bitcoin::bip32::{ChildNumber, DerivationPath, Fingerprint};
use bitcoin::hashes::sha256;
use bitcoin::hashes::Hash;
use bitcoin::key::rand::thread_rng;
use bitcoin::Psbt;
use bitcoin::{secp256k1::SecretKey, Amount, FeeRate, ScriptBuf, Transaction};
use ddk_manager::{error::Error as ManagerError, SimpleSigner};
use lightning::chain::chaininterface::{ConfirmationTarget, FeeEstimator};
use std::collections::HashMap;
use std::fmt::Debug;
use std::future::Future;
use std::pin::Pin;
use std::str::FromStr;
use std::sync::atomic::AtomicU32;
use std::sync::{atomic::Ordering, Arc};
use tokio::sync::{
    mpsc::{channel, Sender},
    oneshot,
};

type FutureResult<'a, T, E> = Pin<Box<dyn Future<Output = std::result::Result<T, E>> + Send + 'a>>;
type Result<T> = std::result::Result<T, WalletError>;

/// We choose this number for the range of child numbers that are used for the DLC key path.
/// This allows for 3400^3 = 39.3 billion possible paths.
/// It is large enough to avoid collisions, but small enough to be practical for a doomsday scenario.
///
/// Recovery would be ~1 week for each contract key with the Xpriv.
const CHILD_NUMBER_RANGE: u32 = 3_400;

/// The minimum change size for the wallet to create in coin selection.
const MIN_CHANGE_SIZE: u64 = 25_000;

/// Wrapper type that adapts DDK's Storage trait to BDK's AsyncWalletPersister interface.
///
/// This wrapper is necessary because BDK requires a persister that implements AsyncWalletPersister,
/// but DDK's Storage trait provides a different interface. The wrapper provides thread safety
/// and interior mutability required by BDK while delegating to the underlying DDK storage.
///
/// # Thread Safety
/// The wrapper uses Arc<dyn Storage> to ensure the storage can be safely shared across threads
/// and provides the necessary interior mutability for BDK operations.
#[derive(Clone, Debug)]
pub struct WalletStorage(Arc<dyn Storage>);

impl AsyncWalletPersister for WalletStorage {
    type Error = WalletError;

    /// Initializes the wallet storage by calling the underlying storage's initialize_bdk method.
    /// This loads any existing wallet state from persistent storage.
    fn initialize<'a>(
        persister: &'a mut Self,
    ) -> FutureResult<'a, bdk_wallet::ChangeSet, Self::Error>
    where
        Self: 'a,
    {
        Box::pin(persister.0.initialize_bdk())
    }

    /// Persists wallet changes to storage by calling the underlying storage's persist_bdk method.
    /// This saves any wallet state changes to persistent storage.
    fn persist<'a>(
        persister: &'a mut Self,
        changeset: &'a bdk_wallet::ChangeSet,
    ) -> FutureResult<'a, (), Self::Error>
    where
        Self: 'a,
    {
        Box::pin(persister.0.persist_bdk(changeset))
    }
}

/// Commands that can be sent to the wallet actor.
///
/// The wallet operates using an actor model where all operations are performed
/// by sending commands through a message channel. Each command includes a oneshot
/// channel for receiving the result, enabling async request/response patterns
/// while maintaining thread safety.
///
/// # Actor Model Benefits
/// - Lock-free operations
/// - Thread-safe concurrent access
/// - Isolation of wallet state
/// - Async operation support
#[derive(Debug)]
pub enum WalletCommand {
    /// Synchronize the wallet with the blockchain
    Sync(oneshot::Sender<Result<()>>),

    /// Get the current wallet balance
    Balance(oneshot::Sender<Balance>),

    /// Generate a new external (receiving) address
    NewExternalAddress(oneshot::Sender<Result<AddressInfo>>),

    /// Generate a new internal (change) address
    NewChangeAddress(oneshot::Sender<Result<AddressInfo>>),

    /// Send a specific amount to an address with the given fee rate
    SendToAddress(Address, Amount, FeeRate, oneshot::Sender<Result<Txid>>),

    /// Send all available funds to an address with the given fee rate
    SendAll(Address, FeeRate, oneshot::Sender<Result<Txid>>),

    /// Get all wallet transactions
    GetTransactions(oneshot::Sender<Result<Vec<Arc<Transaction>>>>),

    /// List all unspent transaction outputs (UTXOs)
    ListUtxos(oneshot::Sender<Result<Vec<LocalOutput>>>),

    /// Get the next derivation index for address generation
    NextDerivationIndex(oneshot::Sender<Result<u32>>),

    /// Sign a specific input in a PSBT (Partially Signed Bitcoin Transaction)
    SignPsbtInput(
        bitcoin::psbt::Psbt,
        usize,
        oneshot::Sender<std::result::Result<Psbt, ManagerError>>,
    ),
}

/// The main wallet implementation that provides Bitcoin functionality for DDK.
///
/// This wallet uses BDK for Bitcoin operations and implements an actor pattern
/// for thread-safe access. It integrates with DDK's storage system and provides
/// all necessary functionality for DLC operations including PSBT signing.
///
/// # Architecture
/// - Uses tokio channels for message passing
/// - Spawns a background task to handle wallet operations  
/// - Provides async API that sends commands to the background task
/// - Integrates with Esplora for blockchain data
/// - Uses BIP84 (native segwit) descriptors
///
/// # Thread Safety
/// The wallet is designed to be thread-safe through the actor model:
/// - All state is isolated in the background task
/// - External access is only through message passing
/// - No shared mutable state between threads
pub struct DlcDevKitWallet {
    /// Channel sender for wallet commands
    sender: Sender<WalletCommand>,
    /// Bitcoin network (mainnet, testnet, regtest)
    network: Network,
    /// Extended private key for the wallet
    xprv: Xpriv,
    /// Secp256k1 context for cryptographic operations
    secp: Secp256k1<All>,
    /// Fingerprint of the wallet
    fingerprint: Fingerprint,
    /// Derivation path for DLC keys
    dlc_path: DerivationPath,
    /// Function to generate external addresses
    address_generator: Option<Arc<dyn AddressGenerator + Send + Sync>>,
    /// Logger
    logger: Arc<Logger>,
}

const MIN_FEERATE: u32 = 253;

/// Helper function to extract the checksum from a descriptor string.
fn extract_descriptor_checksum(descriptor: &str) -> String {
    if let Some(hash_pos) = descriptor.rfind('#') {
        let checksum = &descriptor[hash_pos + 1..];
        // Trim whitespace and take exactly 8 characters (typical checksum length)
        let trimmed = checksum.trim();
        trimmed.chars().take(8).collect()
    } else {
        "unknown".to_string()
    }
}

/// Extracts fingerprint and derivation path from bracketed content in descriptor.
fn extract_descriptor_fingerprint_and_path(descriptor: &str) -> (String, String) {
    if let Some(bracket_start) = descriptor.find('[') {
        if let Some(bracket_end) = descriptor[bracket_start..].find(']') {
            let content = &descriptor[bracket_start + 1..bracket_start + bracket_end];
            if let Some(slash_pos) = content.find('/') {
                return (
                    content[..slash_pos].to_string(),
                    content[slash_pos + 1..].to_string(),
                );
            }
        }
    }
    ("unknown".to_string(), "unknown".to_string())
}

/// Attempts to extract structured information from the error chain.
///
/// Walks the error source chain looking for:
/// 1. Exact descriptor strings in error messages (most reliable)
/// 2. Enum variant names in Debug format (e.g., "KeychainKind::External")
///
/// Returns a tuple of (keychain, descriptor_string) if any matching evidence is found, or None otherwise.
fn extract_structured_error_info(
    error: &dyn std::error::Error,
    external_descriptor_str: &str,
    internal_descriptor_str: &str,
) -> Option<(&'static str, String)> {
    let mut current: Option<&dyn std::error::Error> = Some(error);

    // Walk the error chain
    while let Some(err) = current {
        let error_debug = format!("{:?}", err);
        let error_msg = err.to_string();

        // Check for exact descriptor strings (most reliable indicator)
        // This works even if BDK's error format changes
        if error_msg.contains(external_descriptor_str)
            || error_debug.contains(external_descriptor_str)
        {
            return Some(("external", external_descriptor_str.to_string()));
        }

        if error_msg.contains(internal_descriptor_str)
            || error_debug.contains(internal_descriptor_str)
        {
            return Some(("internal", internal_descriptor_str.to_string()));
        }

        // Try to extract keychain from Debug format enum variants
        if error_debug.contains("KeychainKind::External") {
            return Some(("external", external_descriptor_str.to_string()));
        }

        if error_debug.contains("KeychainKind::Internal") {
            return Some(("internal", internal_descriptor_str.to_string()));
        }

        // Move to next error in chain
        current = err.source();
    }

    None
}

/// Returns true if the error looks like a descriptor mismatch (heuristics-based).
fn is_descriptor_mismatch(
    error: &dyn std::error::Error,
    external_descriptor_str: &str,
    internal_descriptor_str: &str,
) -> bool {
    extract_structured_error_info(error, external_descriptor_str, internal_descriptor_str).is_some()
}

/// Identifies descriptor mismatches in BDK errors and extracts info on which keychain failed.
fn extract_descriptor_info(
    error: &dyn std::error::Error,
    external_descriptor_str: &str,
    internal_descriptor_str: &str,
) -> WalletError {
    // Extract structured information from error chain
    let (keychain, expected_descriptor) =
        extract_structured_error_info(error, external_descriptor_str, internal_descriptor_str)
            .unwrap_or(("external", external_descriptor_str.to_string()));

    // Format expected descriptor info
    let expected = format!(
        "  Checksum: {}",
        extract_descriptor_checksum(&expected_descriptor)
    );

    // Extract stored descriptor info from error message
    // Note: This requires parsing the error message string, but it's necessary
    // to meet the requirement of showing expected vs stored descriptor for comparison
    let error_msg = error.to_string();
    let error_debug = format!("{:?}", error);
    let (stored_checksum, _stored_fingerprint, _stored_path) =
        extract_stored_descriptor_info(&error_msg, &error_debug);
    let stored = format!("  Checksum: {}", stored_checksum);

    WalletError::DescriptorMismatch {
        keychain: keychain.to_string(),
        expected,
        stored,
    }
}

/// Extracts checksum, fingerprint, and derivation path from the stored descriptor
/// in BDK error messages.
fn extract_stored_descriptor_info(error_msg: &str, error_debug: &str) -> (String, String, String) {
    // Try both error message formats
    for text in [error_msg, error_debug] {
        if let Some(loaded_pos) = text.find("loaded ") {
            let after_loaded = &text[loaded_pos + 7..]; // Skip "loaded "

            // Extract the full descriptor string (up to the comma or end)
            let desc_end = after_loaded.find(',').unwrap_or(after_loaded.len());
            let descriptor_str = after_loaded[..desc_end].trim();

            // Try to parse the descriptor using BDK's parser
            if let Ok(descriptor) = descriptor_str.parse::<Descriptor<DescriptorPublicKey>>() {
                // Get the canonical string representation (includes checksum)
                let canonical_str = descriptor.to_string();

                let checksum = extract_descriptor_checksum(&canonical_str);
                let (fingerprint, path) = extract_descriptor_fingerprint_and_path(&canonical_str);

                if checksum != "unknown" || path != "unknown" {
                    return (checksum, fingerprint, path);
                }
            }
        }
    }

    (
        "unknown".to_string(),
        "unknown".to_string(),
        "unknown".to_string(),
    )
}

impl DlcDevKitWallet {
    /// Creates a new DlcDevKitWallet instance.
    ///
    /// This method:
    /// 1. Generates BIP84 descriptors from the seed
    /// 2. Creates or loads the BDK wallet from storage
    /// 3. Sets up Esplora client for blockchain communication
    /// 4. Spawns the wallet actor task
    /// 5. Returns the wallet handle for external use
    ///
    /// # Arguments
    /// * `seed_bytes` - 32-byte seed for wallet derivation
    /// * `esplora_url` - URL of the Esplora server for blockchain data
    /// * `network` - Bitcoin network to use
    /// * `storage` - Storage backend for persistence
    ///
    /// # Returns
    /// A new DlcDevKitWallet instance ready for use
    ///
    /// # Actor Task
    /// The method spawns a background task that:
    /// - Processes incoming wallet commands
    /// - Maintains wallet state
    /// - Handles all BDK operations
    /// - Manages blockchain synchronization
    #[tracing::instrument(name = "wallet", skip_all)]
    pub async fn new(
        seed_bytes: &[u8; 64],
        blockchain: Arc<EsploraClient>,
        network: Network,
        storage: Arc<dyn Storage>,
        address_generator: Option<Arc<dyn AddressGenerator + Send + Sync>>,
        logger: Arc<Logger>,
    ) -> Result<DlcDevKitWallet> {
        let secp = Secp256k1::new();

        let xprv = Xpriv::new_master(network, seed_bytes)?;
        let fingerprint = xprv.fingerprint(&secp);

        let external_descriptor =
            Bip84(xprv, KeychainKind::External).into_wallet_descriptor(&secp, network)?;
        let internal_descriptor =
            Bip84(xprv, KeychainKind::Internal).into_wallet_descriptor(&secp, network)?;

        let mut storage = WalletStorage(storage);

        let load_wallet = Wallet::load()
            .descriptor(KeychainKind::External, Some(external_descriptor.clone()))
            .descriptor(KeychainKind::Internal, Some(internal_descriptor.clone()))
            .extract_keys()
            .check_network(network)
            .load_wallet_async(&mut storage)
            .await
            .map_err(|e| {
                let external_desc_str = external_descriptor.0.to_string();
                let internal_desc_str = internal_descriptor.0.to_string();

                if is_descriptor_mismatch(&e, &external_desc_str, &internal_desc_str) {
                    extract_descriptor_info(&e, &external_desc_str, &internal_desc_str)
                } else {
                    WalletError::WalletPersistanceError(e.to_string())
                }
            })?;

        let mut wallet = match load_wallet {
            Some(w) => w,
            None => Wallet::create(external_descriptor, internal_descriptor)
                .network(network)
                .create_wallet_async(&mut storage)
                .await
                .map_err(|e| WalletError::WalletPersistanceError(e.to_string()))?,
        };

        let dlc_path = DerivationPath::from_str("m/420'/0'/0'")?;

        let (sender, mut receiver) = channel(100);

        let logger_clone = logger.clone();
        tokio::spawn(async move {
            while let Some(command) = receiver.recv().await {
                match command {
                    WalletCommand::Sync(sender) => {
                        let sync = command::sync(
                            &mut wallet,
                            &blockchain,
                            &mut storage,
                            logger_clone.clone(),
                        )
                        .await;
                        let _ = sender.send(sync).map_err(|e| {
                            log_error!(logger_clone, "Error sending sync command. error={:?}", e);
                        });
                    }
                    WalletCommand::Balance(sender) => {
                        let balance = wallet.balance();
                        let _ = sender.send(balance).map_err(|e| {
                            log_error!(
                                logger_clone,
                                "Error sending balance command. error={:?}",
                                e
                            );
                        });
                    }
                    WalletCommand::NewExternalAddress(sender) => {
                        let address = wallet.next_unused_address(KeychainKind::External);
                        let _ = wallet.persist_async(&mut storage).await;
                        let _ = sender.send(Ok(address)).map_err(|e| {
                            log_error!(
                                logger_clone,
                                "Error sending new external address command. error={:?}",
                                e
                            );
                        });
                    }
                    WalletCommand::NewChangeAddress(sender) => {
                        let address = wallet.next_unused_address(KeychainKind::Internal);
                        let _ = wallet.persist_async(&mut storage).await;
                        let _ = sender.send(Ok(address)).map_err(|e| {
                            log_error!(
                                logger_clone,
                                "Error sending new change address command. error={:?}",
                                e
                            );
                        });
                    }
                    WalletCommand::SendToAddress(address, amount, fee_rate, sender) => {
                        let mut txn_builder = wallet.build_tx();
                        txn_builder
                            .add_recipient(address.script_pubkey(), amount)
                            .version(2)
                            .fee_rate(fee_rate);
                        let mut psbt = match txn_builder.finish() {
                            Ok(psbt) => psbt,
                            Err(e) => {
                                let _ = sender.send(Err(WalletError::TxnBuilder(e))).map_err(|e| {
                                    log_error!(
                                        logger_clone,
                                        "Error sending send to address command. error={:?}",
                                        e
                                    );
                                });
                                continue;
                            }
                        };
                        if let Err(e) = wallet.sign(&mut psbt, SignOptions::default()) {
                            let _ = sender.send(Err(WalletError::Signing(e))).map_err(|e| {
                                log_error!(
                                    logger_clone,
                                    "Error sending send to address command. error={:?}",
                                    e
                                );
                            });
                            continue;
                        }
                        let tx = match psbt.extract_tx() {
                            Ok(tx) => tx,
                            Err(_) => {
                                let _ = sender.send(Err(WalletError::ExtractTx)).map_err(|e| {
                                    log_error!(
                                        logger_clone,
                                        "Error sending send to address command. error={:?}",
                                        e
                                    );
                                });
                                continue;
                            }
                        };
                        let txid = tx.compute_txid();
                        if let Err(e) = blockchain.async_client.broadcast(&tx).await {
                            let _ = sender
                                .send(Err(WalletError::Esplora(e.to_string())))
                                .map_err(|e| {
                                    log_error!(
                                        logger_clone,
                                        "Error sending send to address command. error={:?}",
                                        e
                                    );
                                });
                            continue;
                        }
                        let _ = sender.send(Ok(txid)).map_err(|e| {
                            log_error!(
                                logger_clone,
                                "Error sending send to address command. error={:?}",
                                e
                            );
                        });
                    }
                    WalletCommand::SendAll(address, fee_rate, sender) => {
                        let mut tx_builder = wallet.build_tx();
                        tx_builder.fee_rate(fee_rate);
                        tx_builder.drain_wallet();
                        tx_builder.drain_to(address.script_pubkey());
                        let mut psbt = match tx_builder.finish() {
                            Ok(psbt) => psbt,
                            Err(e) => {
                                let _ = sender.send(Err(WalletError::TxnBuilder(e))).map_err(|e| {
                                    log_error!(
                                        logger_clone,
                                        "Error sending send all command. error={:?}",
                                        e
                                    );
                                });
                                continue;
                            }
                        };
                        if let Err(e) = wallet.sign(&mut psbt, SignOptions::default()) {
                            let _ = sender.send(Err(WalletError::Signing(e))).map_err(|e| {
                                log_error!(
                                    logger_clone,
                                    "Error sending send all command. error={:?}",
                                    e
                                );
                            });
                            continue;
                        }
                        let tx = match psbt.extract_tx() {
                            Ok(tx) => tx,
                            Err(_) => {
                                let _ = sender.send(Err(WalletError::ExtractTx)).map_err(|e| {
                                    log_error!(
                                        logger_clone,
                                        "Error sending send all command. error={:?}",
                                        e
                                    );
                                });
                                continue;
                            }
                        };
                        let txid = tx.compute_txid();
                        if let Err(e) = blockchain.async_client.broadcast(&tx).await {
                            let _ = sender
                                .send(Err(WalletError::Esplora(e.to_string())))
                                .map_err(|e| {
                                    log_error!(
                                        logger_clone,
                                        "Error sending send all command. error={:?}",
                                        e
                                    );
                                });
                            continue;
                        }
                        let _ = sender.send(Ok(txid)).map_err(|e| {
                            log_error!(
                                logger_clone,
                                "Error sending send all command. error={:?}",
                                e
                            );
                        });
                    }
                    WalletCommand::GetTransactions(sender) => {
                        let txs = wallet
                            .transactions()
                            .map(|t| t.tx_node.tx)
                            .collect::<Vec<Arc<Transaction>>>();
                        let _ = sender.send(Ok(txs)).map_err(|e| {
                            log_error!(
                                logger_clone,
                                "Error sending get transactions command. error={:?}",
                                e
                            );
                        });
                    }
                    WalletCommand::ListUtxos(sender) => {
                        let utxos = wallet.list_unspent().map(|utxo| utxo.to_owned()).collect();
                        let _ = sender.send(Ok(utxos)).map_err(|e| {
                            log_error!(
                                logger_clone,
                                "Error sending list utxos command. error={:?}",
                                e
                            );
                        });
                    }
                    WalletCommand::NextDerivationIndex(sender) => {
                        let index = wallet.next_derivation_index(KeychainKind::External);
                        let _ = sender.send(Ok(index)).map_err(|e| {
                            log_error!(
                                logger_clone,
                                "Error sending next derivation index command. error={:?}",
                                e
                            );
                        });
                    }
                    WalletCommand::SignPsbtInput(mut psbt, input_index, sender) => {
                        let sign_opts = SignOptions {
                            trust_witness_utxo: true,
                            ..Default::default()
                        };
                        let mut signed_psbt = psbt.clone();
                        if let Err(e) = wallet.sign(&mut signed_psbt, sign_opts) {
                            log_error!(logger_clone, "Could not sign PSBT. error={:?}", e);
                            let _ = sender
                                .send(Err(ManagerError::WalletError(
                                    WalletError::Signing(e).into(),
                                )))
                                .map_err(|e| {
                                    log_error!(
                                        logger_clone,
                                        "Error sending sign psbt input command. error={:?}",
                                        e
                                    );
                                });
                        } else {
                            psbt.inputs[input_index] = signed_psbt.inputs[input_index].clone();
                            let _ = sender.send(Ok(psbt)).map_err(|e| {
                                log_error!(
                                    logger_clone,
                                    "Error sending sign psbt input command. error={:?}",
                                    e
                                );
                            });
                        }
                    }
                }
            }
        });

        Ok(DlcDevKitWallet {
            sender,
            network,
            xprv,
            secp,
            fingerprint,
            dlc_path,
            address_generator,
            logger,
        })
    }

    /// Synchronizes the wallet with the blockchain.
    /// This updates the wallet's UTXO set and transaction history.
    #[tracing::instrument(skip(self))]
    pub async fn sync(&self) -> Result<()> {
        let (tx, rx) = oneshot::channel();
        self.sender.send(WalletCommand::Sync(tx)).await?;
        rx.await.map_err(WalletError::Receiver)?
    }

    /// Returns the wallet's master public key.
    /// Used for identification and key derivation.
    #[tracing::instrument(skip(self))]
    pub fn get_pubkey(&self) -> PublicKey {
        PublicKey::from_secret_key(&self.secp, &self.xprv.private_key)
    }

    /// Retrieves the current wallet balance including confirmed and unconfirmed amounts.
    #[tracing::instrument(skip(self))]
    pub async fn get_balance(&self) -> Result<Balance> {
        let (tx, rx) = oneshot::channel();
        self.sender.send(WalletCommand::Balance(tx)).await?;
        rx.await.map_err(WalletError::Receiver)
    }

    /// Generates a new external (receiving) address.
    /// These addresses are used for receiving funds from external sources.
    ///
    /// WARNING: If you want your custom address generator call
    /// [`address::AddressGenerator::custom_external_address`] instead.
    #[tracing::instrument(skip(self))]
    pub async fn new_external_address(&self) -> Result<AddressInfo> {
        let (tx, rx) = oneshot::channel();
        self.sender
            .send(WalletCommand::NewExternalAddress(tx))
            .await?;
        rx.await.map_err(WalletError::Receiver)?
    }

    /// Generates a new change address.
    /// These addresses are used internally for change outputs.
    ///
    /// WARNING: If you want your custom address generator call
    /// [`address::AddressGenerator::custom_change_address`] instead.
    #[tracing::instrument(skip(self))]
    pub async fn new_change_address(&self) -> Result<AddressInfo> {
        let (tx, rx) = oneshot::channel();
        self.sender
            .send(WalletCommand::NewChangeAddress(tx))
            .await?;
        rx.await.map_err(WalletError::Receiver)?
    }

    /// Sends a specific amount to the given address.
    ///
    /// # Arguments
    /// * `address` - Destination Bitcoin address
    /// * `amount` - Amount to send in satoshis
    /// * `fee_rate` - Fee rate for the transaction
    ///
    /// # Returns
    /// Transaction ID of the sent transaction
    #[tracing::instrument(skip(self))]
    pub async fn send_to_address(
        &self,
        address: Address,
        amount: Amount,
        fee_rate: FeeRate,
    ) -> Result<Txid> {
        let (tx, rx) = oneshot::channel();
        self.sender
            .send(WalletCommand::SendToAddress(address, amount, fee_rate, tx))
            .await?;
        rx.await.map_err(WalletError::Receiver)?
    }

    /// Sends all available funds to the given address.
    ///
    /// # Arguments
    /// * `address` - Destination Bitcoin address
    /// * `fee_rate` - Fee rate for the transaction
    ///
    /// # Returns
    /// Transaction ID of the sent transaction
    #[tracing::instrument(skip(self))]
    pub async fn send_all(&self, address: Address, fee_rate: FeeRate) -> Result<Txid> {
        let (tx, rx) = oneshot::channel();
        self.sender
            .send(WalletCommand::SendAll(address, fee_rate, tx))
            .await?;
        rx.await.map_err(WalletError::Receiver)?
    }

    /// Retrieves all transactions known to the wallet.
    #[tracing::instrument(skip(self))]
    pub async fn get_transactions(&self) -> Result<Vec<Arc<Transaction>>> {
        let (tx, rx) = oneshot::channel();
        self.sender.send(WalletCommand::GetTransactions(tx)).await?;
        rx.await.map_err(WalletError::Receiver)?
    }

    /// Lists all unspent transaction outputs (UTXOs) in the wallet.
    #[tracing::instrument(skip(self))]
    pub async fn list_utxos(&self) -> Result<Vec<LocalOutput>> {
        let (tx, rx) = oneshot::channel();
        self.sender.send(WalletCommand::ListUtxos(tx)).await?;
        rx.await.map_err(WalletError::Receiver)?
    }

    /// Signs a specific input in a PSBT for DLC operations.
    ///
    /// This method is used internally by the DLC manager to sign
    /// DLC-related transactions such as funding transactions.
    ///
    /// # Arguments
    /// * `psbt` - The PSBT to sign
    /// * `input_index` - Index of the input to sign
    #[tracing::instrument(skip(self))]
    async fn sign_psbt_input(
        &self,
        psbt: &mut bitcoin::psbt::Psbt,
        input_index: usize,
    ) -> std::result::Result<(), ManagerError> {
        let (tx, rx) = oneshot::channel();
        self.sender
            .send(WalletCommand::SignPsbtInput(psbt.clone(), input_index, tx))
            .await
            .map_err(|e| ManagerError::WalletError(Box::new(WalletError::Sender(e))))?;
        let signed_psbt_received = rx
            .await
            .map_err(|e| ManagerError::WalletError(Box::new(WalletError::Receiver(e))))?;

        *psbt = signed_psbt_received?;
        Ok(())
    }

    /// Converts a 32-byte key ID into hierarchical indices for derivation paths.
    ///
    /// This function takes a 32-byte key ID and splits it into three 4-byte
    /// arrays, which are then used to calculate indices for three levels of
    /// derivation paths. The indices are calculated using modulo arithmetic
    /// to ensure they fall within the range of 0 to 3399.
    fn key_id_to_hierarchical_indices(&self, key_id: [u8; 32]) -> (u32, u32, u32) {
        let level_1 = [key_id[0], key_id[1], key_id[2], key_id[3]];
        let level_2 = [key_id[4], key_id[5], key_id[6], key_id[7]];
        let level_3 = [key_id[8], key_id[9], key_id[10], key_id[11]];

        let level_1_index = u32::from_be_bytes(level_1) % CHILD_NUMBER_RANGE;
        let level_2_index = u32::from_be_bytes(level_2) % CHILD_NUMBER_RANGE;
        let level_3_index = u32::from_be_bytes(level_3) % CHILD_NUMBER_RANGE;

        // Total combination space: 3400 × 3400 × 3400 = ~39.3 billion possible paths
        (level_1_index, level_2_index, level_3_index)
    }

    fn get_hierarchical_derivation_path(&self, key_id: [u8; 32]) -> Result<DerivationPath> {
        let (level_1_index, level_2_index, level_3_index) =
            self.key_id_to_hierarchical_indices(key_id);
        let child_one = ChildNumber::from_normal_idx(level_1_index)
            .map_err(|_| WalletError::InvalidDerivationIndex)?;
        let child_two = ChildNumber::from_normal_idx(level_2_index)
            .map_err(|_| WalletError::InvalidDerivationIndex)?;
        let child_three = ChildNumber::from_normal_idx(level_3_index)
            .map_err(|_| WalletError::InvalidDerivationIndex)?;

        let path = self.dlc_path.clone();
        let full_path = path.extend([child_one, child_two, child_three]);

        Ok(full_path)
    }

    fn apply_hardening_to_base_key(
        &self,
        base_key: &SecretKey,
        level_1: u32,
        level_2: u32,
        level_3: u32,
    ) -> Result<SecretKey> {
        let mut hardening_input = Vec::new();
        hardening_input.extend_from_slice(self.fingerprint.as_bytes());
        hardening_input.extend_from_slice(&base_key.secret_bytes());
        hardening_input.extend_from_slice(&level_1.to_be_bytes());
        hardening_input.extend_from_slice(&level_2.to_be_bytes());
        hardening_input.extend_from_slice(&level_3.to_be_bytes());

        let hardened_hash = sha256::Hash::hash(&hardening_input);

        SecretKey::from_slice(hardened_hash.as_ref()).map_err(|_| WalletError::InvalidSecretKey)
    }

    #[tracing::instrument(skip(self, key_id))]
    fn derive_secret_key_from_key_id(&self, key_id: [u8; 32]) -> Result<SecretKey> {
        let derivation_path = self.get_hierarchical_derivation_path(key_id)?;

        let base_secret_key = self.xprv.derive_priv(&self.secp, &derivation_path)?;

        let (level_1, level_2, level_3) = self.key_id_to_hierarchical_indices(key_id);

        let hardened_key = self.apply_hardening_to_base_key(
            &base_secret_key.private_key,
            level_1,
            level_2,
            level_3,
        )?;

        Ok(hardened_key)
    }
}

/// Implementation of Lightning's FeeEstimator trait for the wallet.
/// Provides fee estimation for DLC operations based on confirmation targets.
impl FeeEstimator for DlcDevKitWallet {
    /// Returns the estimated fee rate in satoshis per 1000 weight units.
    /// Used by the DLC manager to estimate fees for funding transactions.
    #[tracing::instrument(skip(self))]
    fn get_est_sat_per_1000_weight(&self, confirmation_target: ConfirmationTarget) -> u32 {
        let fees = fee_estimator();
        fees.get(&confirmation_target)
            .unwrap()
            .load(Ordering::Acquire)
    }
}

/// Implementation of DDK manager's ContractSignerProvider trait.
/// Provides cryptographic signing capabilities for DLC contracts.
impl ddk_manager::ContractSignerProvider for DlcDevKitWallet {
    type Signer = SimpleSigner;

    /// Generates a deterministic key ID for contract signing.
    ///
    /// This method creates a unique key identifier for each contract by hashing
    /// the temporary contract ID with random bytes. The resulting key ID is used
    /// to derive signing keys for the specific contract.
    ///
    /// # Arguments
    /// * `_is_offer_party` - Whether this party is the offer party (currently unused)
    /// * `temp_id` - Temporary contract ID from the DLC protocol
    ///
    /// # Returns
    /// A 32-byte key ID for the contract
    #[tracing::instrument(skip(self))]
    fn derive_signer_key_id(&self, _is_offer_party: bool, temp_id: [u8; 32]) -> [u8; 32] {
        let mut key_id_input = Vec::new();

        key_id_input.extend_from_slice(self.fingerprint.as_bytes());
        key_id_input.extend_from_slice(&temp_id);
        key_id_input.extend_from_slice(b"CONTRACT_SIGNER_KEY_ID_V0");

        let key_id_hash = sha256::Hash::hash(&key_id_input);
        key_id_hash.to_byte_array()
    }

    /// Creates a contract signer from a key ID.
    ///
    /// Takes the key ID generated by `derive_signer_key_id` and creates a
    /// SimpleSigner that can sign transactions for the specific contract.
    ///
    /// # Arguments
    /// * `key_id` - The key ID to derive the signer from
    ///
    /// # Returns
    /// A SimpleSigner configured for the contract
    #[tracing::instrument(skip(self, key_id))]
    fn derive_contract_signer(
        &self,
        key_id: [u8; 32],
    ) -> std::result::Result<Self::Signer, ManagerError> {
        let secret_key = self
            .derive_secret_key_from_key_id(key_id)
            .map_err(|e| ManagerError::WalletError(Box::new(e)))?;

        Ok(SimpleSigner::new(secret_key))
    }

    /// Gets a secret key for a given public key.
    /// Currently unimplemented as it's only used for channel operations.
    fn get_secret_key_for_pubkey(
        &self,
        _pubkey: &PublicKey,
    ) -> std::result::Result<SecretKey, ManagerError> {
        unreachable!("get_secret_key_for_pubkey is only used in channels.")
    }

    /// Generates a new secret key.
    /// Currently unimplemented as it's only used for channel operations.
    fn get_new_secret_key(&self) -> std::result::Result<SecretKey, ManagerError> {
        unreachable!("get_new_secret_key is only used for channels")
    }
}

/// Implementation of DDK manager's Wallet trait.
/// Provides the wallet interface required by the DLC manager for contract operations.
#[async_trait::async_trait]
impl ddk_manager::Wallet for DlcDevKitWallet {
    /// Gets a new external address for receiving funds.
    /// Used by the DLC manager when creating funding transactions.
    async fn get_new_address(&self) -> std::result::Result<bitcoin::Address, ManagerError> {
        if let Some(address_generator) = &self.address_generator {
            let address = address_generator
                .custom_external_address()
                .await
                .map_err(wallet_err_to_manager_err)?;
            return Ok(address);
        }

        let address = self
            .new_external_address()
            .await
            .map_err(wallet_err_to_manager_err)?;

        log_info!(
            self.logger.clone(),
            "Revealed new address for contract. address={}",
            address.address.to_string()
        );
        Ok(address.address)
    }

    /// Gets a new change address for transaction outputs.
    /// Used by the DLC manager for change outputs in DLC transactions.
    async fn get_new_change_address(&self) -> std::result::Result<bitcoin::Address, ManagerError> {
        if let Some(address_generator) = &self.address_generator {
            let address = address_generator
                .custom_change_address()
                .await
                .map_err(wallet_err_to_manager_err)?;
            return Ok(address);
        }

        let address = self
            .new_change_address()
            .await
            .map_err(wallet_err_to_manager_err)?;

        log_info!(
            self.logger.clone(),
            "Revealed new change address for contract. address={}",
            address.address.to_string()
        );
        Ok(address.address)
    }

    /// Signs a specific input in a PSBT.
    /// This is the main interface used by the DLC manager to sign DLC-related transactions.
    async fn sign_psbt_input(
        &self,
        psbt: &mut bitcoin::psbt::Psbt,
        input_index: usize,
    ) -> std::result::Result<(), ManagerError> {
        self.sign_psbt_input(psbt, input_index).await
    }

    /// Unreserves UTXOs that were previously reserved for a transaction.
    /// Currently a no-op as UTXO reservation is not implemented.
    fn unreserve_utxos(
        &self,
        _outpoints: &[bitcoin::OutPoint],
    ) -> std::result::Result<(), ManagerError> {
        Ok(())
    }

    /// Imports an address into the wallet for monitoring.
    /// Currently a no-op as address import is not needed.
    fn import_address(&self, _address: &bitcoin::Address) -> std::result::Result<(), ManagerError> {
        Ok(())
    }

    /// Selects UTXOs for a specific amount and fee rate.
    ///
    /// This method is used by the DLC manager to select appropriate UTXOs
    /// for funding DLC transactions. It performs coin selection based on the
    /// requested amount and fee rate.
    ///
    /// # Arguments
    /// * `amount` - The amount of Bitcoin needed
    /// * `fee_rate` - The fee rate for the transaction
    /// * `_lock_utxos` - Whether to lock the selected UTXOs (currently unused)
    ///
    /// # Returns
    /// A vector of UTXOs that can cover the required amount plus fees
    #[tracing::instrument(skip(self))]
    async fn get_utxos_for_amount(
        &self,
        amount: Amount,
        fee_rate: u64,
        _lock_utxos: bool,
    ) -> std::result::Result<Vec<ddk_manager::Utxo>, ManagerError> {
        let local_utxos = self.list_utxos().await.map_err(wallet_err_to_manager_err)?;

        let utxos = local_utxos
            .iter()
            .map(|utxo| WeightedUtxo {
                satisfaction_weight: utxo.txout.weight(),
                utxo: Utxo::Local(utxo.clone()),
            })
            .collect::<Vec<WeightedUtxo>>();

        let selected_utxos = BranchAndBoundCoinSelection::new(MIN_CHANGE_SIZE, SingleRandomDraw)
            .coin_select(
                vec![],
                utxos,
                FeeRate::from_sat_per_vb_unchecked(fee_rate),
                amount,
                ScriptBuf::new().as_script(),
                &mut thread_rng(),
            )
            .map_err(|e| ManagerError::WalletError(Box::new(e)))?;

        let dlc_utxos = selected_utxos
            .selected
            .iter()
            .map(|utxo| {
                let address =
                    Address::from_script(&utxo.txout().script_pubkey, self.network).unwrap();
                ddk_manager::Utxo {
                    tx_out: utxo.txout().clone(),
                    outpoint: utxo.outpoint(),
                    address,
                    redeem_script: ScriptBuf::new(),
                    reserved: false,
                }
            })
            .collect();

        Ok(dlc_utxos)
    }
}

/// Creates a fee estimator with predefined fee rates for different confirmation targets.
///
/// This function sets up fee estimation for different urgency levels:
/// - High Priority: For immediate confirmation
/// - Normal: For confirmation within a few blocks  
/// - Background: For non-urgent transactions
///
/// Returns a HashMap mapping confirmation targets to atomic fee rates.
fn fee_estimator() -> HashMap<ConfirmationTarget, AtomicU32> {
    let mut fees: HashMap<ConfirmationTarget, AtomicU32> = HashMap::new();
    fees.insert(ConfirmationTarget::UrgentOnChainSweep, AtomicU32::new(5000));
    fees.insert(
        ConfirmationTarget::MinAllowedAnchorChannelRemoteFee,
        AtomicU32::new(25 * 250),
    );
    fees.insert(
        ConfirmationTarget::MinAllowedAnchorChannelRemoteFee,
        AtomicU32::new(MIN_FEERATE),
    );
    fees.insert(
        ConfirmationTarget::MinAllowedNonAnchorChannelRemoteFee,
        AtomicU32::new(MIN_FEERATE),
    );
    fees.insert(
        ConfirmationTarget::AnchorChannelFee,
        AtomicU32::new(MIN_FEERATE),
    );
    fees.insert(
        ConfirmationTarget::NonAnchorChannelFee,
        AtomicU32::new(2000),
    );
    fees.insert(
        ConfirmationTarget::ChannelCloseMinimum,
        AtomicU32::new(MIN_FEERATE),
    );
    fees
}

impl Debug for DlcDevKitWallet {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "DlcDevKitWallet")?;
        write!(f, " fingerprint: {:?}", self.fingerprint)?;
        write!(f, " network: {:?}", self.network)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::{collections::HashSet, str::FromStr, sync::Arc, time::Duration};

    use crate::chain::EsploraClient;
    use crate::logger::{LogLevel, Logger};
    use crate::storage::memory::MemoryStorage;
    use bitcoin::{
        address::NetworkChecked,
        bip32::ChildNumber,
        key::rand::Fill,
        secp256k1::{PublicKey, SecretKey},
        Address, AddressType, Amount, FeeRate, Network,
    };
    use bitcoincore_rpc::RpcApi;
    use ddk_manager::{ContractSigner, ContractSignerProvider};

    use super::DlcDevKitWallet;

    async fn create_wallet() -> DlcDevKitWallet {
        let esplora = std::env::var("ESPLORA_HOST").expect("ESPLORA_HOST must be set");
        let storage = Arc::new(MemoryStorage::new());
        let logger = Arc::new(Logger::console(
            "console_logger".to_string(),
            LogLevel::Info,
        ));
        let esplora =
            Arc::new(EsploraClient::new(&esplora, Network::Regtest, logger.clone()).unwrap());
        let mut entropy = [0u8; 64];
        entropy
            .try_fill(&mut bitcoin::key::rand::thread_rng())
            .unwrap();
        DlcDevKitWallet::new(
            &entropy,
            esplora,
            Network::Regtest,
            storage.clone(),
            None,
            logger.clone(),
        )
        .await
        .unwrap()
    }

    fn generate_blocks(num: u64) {
        let bitcoind =
            std::env::var("BITCOIND_HOST").unwrap_or("http://localhost:18443".to_string());
        let user = std::env::var("BITCOIND_USER").expect("BITCOIND_USER must be set");
        let pass = std::env::var("BITCOIND_PASS").expect("BITCOIND_PASS must be set");
        let auth = bitcoincore_rpc::Auth::UserPass(user, pass);
        let client = bitcoincore_rpc::Client::new(&bitcoind, auth).unwrap();
        let previous_height = client.get_block_count().unwrap();

        let address = client.get_new_address(None, None).unwrap().assume_checked();
        client.generate_to_address(num, &address).unwrap();
        let mut cur_block_height = previous_height;
        while cur_block_height < previous_height + num {
            std::thread::sleep(Duration::from_secs(5));
            cur_block_height = client.get_block_count().unwrap();
        }
    }

    fn fund_address(address: &Address<NetworkChecked>) {
        let bitcoind =
            std::env::var("BITCOIND_HOST").unwrap_or("http://localhost:18443".to_string());
        let user = std::env::var("BITCOIND_USER").expect("BITCOIND_USER must be set");
        let pass = std::env::var("BITCOIND_PASS").expect("BITCOIND_PASS must be set");
        let auth = bitcoincore_rpc::Auth::UserPass(user, pass);
        let client = bitcoincore_rpc::Client::new(&bitcoind, auth).unwrap();
        client
            .send_to_address(
                address,
                Amount::from_btc(1.0).unwrap(),
                None,
                None,
                None,
                None,
                None,
                None,
            )
            .unwrap();
        generate_blocks(5)
    }

    #[tokio::test]
    async fn address_is_p2wpkh() {
        let test = create_wallet().await;
        let address = test.new_external_address().await.unwrap();
        assert_eq!(address.address.address_type().unwrap(), AddressType::P2wpkh)
    }

    #[tokio::test]
    async fn derive_contract_signer() {
        let test = create_wallet().await;
        let mut temp_key_id = [0u8; 32];
        temp_key_id
            .try_fill(&mut bitcoin::key::rand::thread_rng())
            .unwrap();
        let gen_key_id = test.derive_signer_key_id(true, temp_key_id);
        let key_info = test.derive_contract_signer(gen_key_id);
        assert!(key_info.is_ok())
    }

    #[tokio::test]
    async fn send_all() {
        let wallet = create_wallet().await;
        let address = match wallet.network {
            Network::Regtest => "bcrt1qt0yrvs7qx8guvpqsx8u9mypz6t4zr3pxthsjkm",
            Network::Signet => "bcrt1q7h9uzwvyw29vrpujp69l7kce7e5w98mpn8kwsp",
            _ => "bcrt1qt0yrvs7qx8guvpqsx8u9mypz6t4zr3pxthsjkm",
        };
        let addr_one = wallet.new_external_address().await.unwrap().address;
        let addr_two = wallet.new_external_address().await.unwrap().address;
        fund_address(&addr_one);
        fund_address(&addr_two);
        wallet.sync().await.unwrap();
        let balance = wallet.get_balance().await.unwrap();
        assert!(balance.confirmed > Amount::ZERO);
        wallet
            .send_all(
                Address::from_str(address).unwrap().assume_checked(),
                FeeRate::from_sat_per_vb(1).unwrap(),
            )
            .await
            .unwrap();
        generate_blocks(5);
        wallet.sync().await.unwrap();
        let balance = wallet.get_balance().await.unwrap();
        assert!(balance.confirmed == Amount::ZERO)
    }

    #[tokio::test]
    async fn derive_secret_key_from_key_id() {
        let wallet = create_wallet().await;
        let mut temp_key_id = [0u8; 32];
        temp_key_id
            .try_fill(&mut bitcoin::key::rand::thread_rng())
            .unwrap();

        let key_id = wallet.derive_signer_key_id(true, temp_key_id);
        let secret_key = wallet.derive_secret_key_from_key_id(key_id);
        assert!(secret_key.is_ok());
    }

    #[tokio::test]
    async fn key_id_to_hierarchical_indices_deterministic() {
        let wallet = create_wallet().await;

        // Test with a known key_id
        let key_id = [
            0x12, 0x34, 0x56, 0x78, // level_1: should give same result each time
            0x9A, 0xBC, 0xDE, 0xF0, // level_2
            0x11, 0x22, 0x33, 0x44, // level_3
            0x55, 0x66, 0x77, 0x88, // unused bytes
            0x99, 0xAA, 0xBB, 0xCC, 0xDD, 0xEE, 0xFF, 0x00, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06,
            0x07, 0x08,
        ];

        let (level1_1, level2_1, level3_1) = wallet.key_id_to_hierarchical_indices(key_id);
        let (level1_2, level2_2, level3_2) = wallet.key_id_to_hierarchical_indices(key_id);

        // Should be deterministic - same input produces same output
        assert_eq!(level1_1, level1_2);
        assert_eq!(level2_1, level2_2);
        assert_eq!(level3_1, level3_2);

        // Verify indices are within expected range
        assert!(level1_1 < 3400);
        assert!(level2_1 < 3400);
        assert!(level3_1 < 3400);

        // Calculate expected values manually to verify correctness
        let expected_level1 = u32::from_be_bytes([0x12, 0x34, 0x56, 0x78]) % 3400;
        let expected_level2 = u32::from_be_bytes([0x9A, 0xBC, 0xDE, 0xF0]) % 3400;
        let expected_level3 = u32::from_be_bytes([0x11, 0x22, 0x33, 0x44]) % 3400;

        assert_eq!(level1_1, expected_level1);
        assert_eq!(level2_1, expected_level2);
        assert_eq!(level3_1, expected_level3);
    }

    #[tokio::test]
    async fn key_id_to_hierarchical_indices_distribution() {
        let wallet = create_wallet().await;
        let mut level1_values = HashSet::new();
        let mut level2_values = HashSet::new();
        let mut level3_values = HashSet::new();

        // Test with 1000 different key_ids to check distribution
        for i in 0..1000u32 {
            let mut key_id = [0u8; 32];
            // Create variation in the first 12 bytes
            key_id[0..4].copy_from_slice(&i.to_be_bytes());
            key_id[4..8].copy_from_slice(&(i.wrapping_mul(7919)).to_be_bytes());
            key_id[8..12].copy_from_slice(&(i.wrapping_mul(104729)).to_be_bytes());

            let (level1, level2, level3) = wallet.key_id_to_hierarchical_indices(key_id);
            level1_values.insert(level1);
            level2_values.insert(level2);
            level3_values.insert(level3);
        }

        // Should have good distribution - expect most values to be unique for small sample
        assert!(
            level1_values.len() > 900,
            "Level 1 distribution too poor: {} unique values",
            level1_values.len()
        );
        assert!(
            level2_values.len() > 900,
            "Level 2 distribution too poor: {} unique values",
            level2_values.len()
        );
        assert!(
            level3_values.len() > 900,
            "Level 3 distribution too poor: {} unique values",
            level3_values.len()
        );
    }

    #[tokio::test]
    async fn get_hierarchical_derivation_path() {
        let wallet = create_wallet().await;

        let key_id = [1u8; 32]; // Simple test key_id
        let path = wallet
            .get_hierarchical_derivation_path(key_id)
            .expect("Should create valid derivation path");

        // Verify the path has the correct structure
        // Should be: m/9999'/0'/0'/level1/level2/level3 (6 components total)
        assert_eq!(path.len(), 6);

        // Verify base path components (hardened derivation)
        assert_eq!(path[0], ChildNumber::from_hardened_idx(420).unwrap());
        assert_eq!(path[1], ChildNumber::from_hardened_idx(0).unwrap());
        assert_eq!(path[2], ChildNumber::from_hardened_idx(0).unwrap());

        // The last three should be normal (non-hardened) derivation
        assert!(!path[3].is_hardened());
        assert!(!path[4].is_hardened());
        assert!(!path[5].is_hardened());

        // Verify indices match what we expect from key_id_to_hierarchical_indices
        let (expected_level1, expected_level2, expected_level3) =
            wallet.key_id_to_hierarchical_indices(key_id);
        assert_eq!(
            path[3],
            ChildNumber::from_normal_idx(expected_level1).unwrap()
        );
        assert_eq!(
            path[4],
            ChildNumber::from_normal_idx(expected_level2).unwrap()
        );
        assert_eq!(
            path[5],
            ChildNumber::from_normal_idx(expected_level3).unwrap()
        );
    }

    #[tokio::test]
    async fn apply_hardening_to_base_key_deterministic() {
        let wallet = create_wallet().await;

        // Create a test base key
        let base_key = SecretKey::from_slice(&[0x42; 32]).expect("Valid secret key");
        let level1 = 123;
        let level2 = 456;
        let level3 = 789;

        // Apply hardening multiple times
        let hardened1 = wallet
            .apply_hardening_to_base_key(&base_key, level1, level2, level3)
            .expect("Hardening should succeed");
        let hardened2 = wallet
            .apply_hardening_to_base_key(&base_key, level1, level2, level3)
            .expect("Hardening should succeed");

        // Should be deterministic
        assert_eq!(hardened1.secret_bytes(), hardened2.secret_bytes());

        // Should be different from the base key
        assert_ne!(hardened1.secret_bytes(), base_key.secret_bytes());
    }

    #[tokio::test]
    async fn apply_hardening_different_inputs_produce_different_outputs() {
        let wallet = create_wallet().await;
        let base_key = SecretKey::from_slice(&[0x42; 32]).expect("Valid secret key");

        // Test different level combinations produce different results
        let hardened1 = wallet
            .apply_hardening_to_base_key(&base_key, 100, 200, 300)
            .unwrap();
        let hardened2 = wallet
            .apply_hardening_to_base_key(&base_key, 100, 200, 301)
            .unwrap(); // level3 different
        let hardened3 = wallet
            .apply_hardening_to_base_key(&base_key, 100, 201, 300)
            .unwrap(); // level2 different
        let hardened4 = wallet
            .apply_hardening_to_base_key(&base_key, 101, 200, 300)
            .unwrap(); // level1 different

        // All should be different
        assert_ne!(hardened1.secret_bytes(), hardened2.secret_bytes());
        assert_ne!(hardened1.secret_bytes(), hardened3.secret_bytes());
        assert_ne!(hardened1.secret_bytes(), hardened4.secret_bytes());
        assert_ne!(hardened2.secret_bytes(), hardened3.secret_bytes());
        assert_ne!(hardened2.secret_bytes(), hardened4.secret_bytes());
        assert_ne!(hardened3.secret_bytes(), hardened4.secret_bytes());
    }

    #[tokio::test]
    async fn derive_secret_key_from_key_id_complete_flow() {
        let wallet = create_wallet().await;

        let key_id = [0x33; 32]; // Test key_id
        let secret_key1 = wallet
            .derive_secret_key_from_key_id(key_id)
            .expect("Should derive secret key successfully");
        let secret_key2 = wallet
            .derive_secret_key_from_key_id(key_id)
            .expect("Should derive secret key successfully");

        // Should be deterministic
        assert_eq!(secret_key1.secret_bytes(), secret_key2.secret_bytes());

        // Verify the secret key is valid for secp256k1
        let public_key = PublicKey::from_secret_key(&wallet.secp, &secret_key1);
        assert!(public_key
            .verify(
                &wallet.secp,
                &bitcoin::secp256k1::Message::from_digest([0u8; 32]),
                &wallet.secp.sign_ecdsa(
                    &bitcoin::secp256k1::Message::from_digest([0u8; 32]),
                    &secret_key1
                )
            )
            .is_ok());
    }

    #[tokio::test]
    async fn derive_signer_key_id_deterministic() {
        let wallet = create_wallet().await;

        let temp_id = [0x55; 32];

        // Test both offer party values produce same result (since _is_offer_party is unused)
        let key_id1 = wallet.derive_signer_key_id(true, temp_id);
        let key_id2 = wallet.derive_signer_key_id(false, temp_id);
        let key_id3 = wallet.derive_signer_key_id(true, temp_id); // repeat with same params

        assert_eq!(key_id1, key_id2); // is_offer_party doesn't affect result
        assert_eq!(key_id1, key_id3); // deterministic
    }

    #[tokio::test]
    async fn derive_signer_key_id_different_temps_produce_different_keys() {
        let wallet = create_wallet().await;

        let temp_id1 = [0x11; 32];
        let temp_id2 = [0x22; 32];

        let key_id1 = wallet.derive_signer_key_id(true, temp_id1);
        let key_id2 = wallet.derive_signer_key_id(true, temp_id2);

        // Different temp_ids should produce different key_ids
        assert_ne!(key_id1, key_id2);
    }

    #[tokio::test]
    async fn derive_signer_key_id_includes_fingerprint() {
        let wallet1 = create_wallet().await;
        let wallet2 = create_wallet().await;

        let temp_id = [0x99; 32];

        // Same temp_id should produce different key_ids for different wallets
        let key_id1 = wallet1.derive_signer_key_id(true, temp_id);
        let key_id2 = wallet2.derive_signer_key_id(true, temp_id);

        assert_ne!(
            key_id1, key_id2,
            "Different wallets should produce different key_ids for same temp_id"
        );
    }

    #[tokio::test]
    async fn derive_contract_signer_creates_valid_signer() {
        let wallet = create_wallet().await;

        let temp_id = [0x77; 32];
        let key_id = wallet.derive_signer_key_id(true, temp_id);
        let signer = wallet
            .derive_contract_signer(key_id)
            .expect("Should create valid signer");

        // Verify the signer has a valid public key
        let public_key = signer.get_public_key(&wallet.secp).unwrap();

        // The public key should be valid (this would panic if invalid)
        assert!(public_key
            .verify(
                &wallet.secp,
                &bitcoin::secp256k1::Message::from_digest([0u8; 32]),
                &wallet.secp.sign_ecdsa(
                    &bitcoin::secp256k1::Message::from_digest([0u8; 32]),
                    &signer.get_secret_key().unwrap()
                )
            )
            .is_ok());
    }

    #[tokio::test]
    async fn full_workflow_deterministic() {
        let wallet = create_wallet().await;

        let temp_id = [0xAB; 32];

        // Full workflow: temp_id -> key_id -> signer
        let key_id = wallet.derive_signer_key_id(true, temp_id);
        let signer1 = wallet.derive_contract_signer(key_id).unwrap();

        // Repeat the workflow
        let key_id2 = wallet.derive_signer_key_id(true, temp_id);
        let signer2 = wallet.derive_contract_signer(key_id2).unwrap();

        // Everything should be identical
        assert_eq!(key_id, key_id2);
        assert_eq!(
            signer1.get_public_key(&wallet.secp).unwrap(),
            signer2.get_public_key(&wallet.secp).unwrap()
        );
    }

    #[tokio::test]
    async fn different_temp_ids_produce_different_signers() {
        let wallet = create_wallet().await;

        let temp_id1 = [0x01; 32];
        let temp_id2 = [0x02; 32];

        let key_id1 = wallet.derive_signer_key_id(true, temp_id1);
        let key_id2 = wallet.derive_signer_key_id(true, temp_id2);
        let signer1 = wallet.derive_contract_signer(key_id1).unwrap();
        let signer2 = wallet.derive_contract_signer(key_id2).unwrap();

        // Different temp_ids should produce different signers
        assert_ne!(key_id1, key_id2);
        assert_ne!(
            signer1.get_public_key(&wallet.secp).unwrap(),
            signer2.get_public_key(&wallet.secp).unwrap()
        );
    }

    #[tokio::test]
    async fn hierarchical_indices_bounds() {
        let wallet = create_wallet().await;

        // Test edge cases with extreme values
        let max_key_id = [0xFF; 32];
        let min_key_id = [0x00; 32];

        let (max_l1, max_l2, max_l3) = wallet.key_id_to_hierarchical_indices(max_key_id);
        let (min_l1, min_l2, min_l3) = wallet.key_id_to_hierarchical_indices(min_key_id);

        // All indices should be within bounds
        assert!(max_l1 < 3400);
        assert!(max_l2 < 3400);
        assert!(max_l3 < 3400);
        assert!(min_l1 < 3400);
        assert!(min_l2 < 3400);
        assert!(min_l3 < 3400);

        // Min key_id should produce all zeros
        assert_eq!(min_l1, 0);
        assert_eq!(min_l2, 0);
        assert_eq!(min_l3, 0);
    }

    #[tokio::test]
    async fn collision_resistance_sample() {
        let wallet = create_wallet().await;
        let mut key_ids = HashSet::new();
        let mut public_keys = HashSet::new();

        // Generate 1000 contracts and verify no collisions
        for i in 0..1000u32 {
            let mut temp_id = [0u8; 32];
            temp_id[0..4].copy_from_slice(&i.to_be_bytes());

            let key_id = wallet.derive_signer_key_id(true, temp_id);
            let signer = wallet.derive_contract_signer(key_id).unwrap();
            let public_key = signer.get_public_key(&wallet.secp).unwrap();

            // Verify no collisions in key_ids or public keys
            assert!(
                key_ids.insert(key_id),
                "Key ID collision detected at iteration {}",
                i
            );
            assert!(
                public_keys.insert(public_key),
                "Public key collision detected at iteration {}",
                i
            );
        }

        assert_eq!(key_ids.len(), 1000);
        assert_eq!(public_keys.len(), 1000);
    }

    #[tokio::test]
    async fn recovery_scenario_simulation() {
        let wallet = create_wallet().await;

        // Simulate creating a contract
        let temp_id = [0xDE, 0xAD, 0xBE, 0xEF].repeat(8).try_into().unwrap();
        let key_id = wallet.derive_signer_key_id(true, temp_id);
        let original_signer = wallet.derive_contract_signer(key_id).unwrap();
        let target_public_key = original_signer.get_public_key(&wallet.secp).unwrap();

        // Simulate recovery: we know the target public key and need to find the secret key
        // In practice, this would involve scanning, but for testing we'll verify direct recovery
        let recovered_signer = wallet.derive_contract_signer(key_id).unwrap();

        assert_eq!(
            original_signer.get_public_key(&wallet.secp).unwrap(),
            recovered_signer.get_public_key(&wallet.secp).unwrap()
        );

        // Also test that we can recover from just the temp_id
        let recovered_key_id = wallet.derive_signer_key_id(true, temp_id);
        let temp_id_recovered_signer = wallet.derive_contract_signer(recovered_key_id).unwrap();

        assert_eq!(key_id, recovered_key_id);
        assert_eq!(
            target_public_key,
            temp_id_recovered_signer
                .get_public_key(&wallet.secp)
                .unwrap()
        );
    }

    struct DummyAddressGenerator;
    #[async_trait::async_trait]
    impl super::address::AddressGenerator for DummyAddressGenerator {
        async fn custom_external_address(&self) -> Result<Address, crate::error::WalletError> {
            Ok(
                Address::from_str("bcrt1qgnflehdvm85l5qmhf887lklda43ynh6tlx4ly0")
                    .unwrap()
                    .assume_checked(),
            )
        }

        async fn custom_change_address(&self) -> Result<Address, crate::error::WalletError> {
            Ok(
                Address::from_str("bcrt1qqhxq8mgmlx3njn3kcx3zmxzuyarcrh5huhm55t")
                    .unwrap()
                    .assume_checked(),
            )
        }
    }

    #[tokio::test]
    async fn custom_address_generator() {
        use ddk_manager::Wallet;

        let address = Address::from_str("bcrt1qgnflehdvm85l5qmhf887lklda43ynh6tlx4ly0")
            .unwrap()
            .assume_checked();

        let change_address = Address::from_str("bcrt1qqhxq8mgmlx3njn3kcx3zmxzuyarcrh5huhm55t")
            .unwrap()
            .assume_checked();

        let logger = Arc::new(Logger::console(
            "console_logger".to_string(),
            LogLevel::Info,
        ));
        let esplora_host = std::env::var("ESPLORA_HOST").expect("ESPLORA_HOST must be set");
        let esplora =
            Arc::new(EsploraClient::new(&esplora_host, Network::Regtest, logger.clone()).unwrap());

        let mut seed = [0u8; 64];
        seed.try_fill(&mut bitcoin::key::rand::thread_rng())
            .unwrap();

        let memory_storage = Arc::new(MemoryStorage::new());
        let wallet = DlcDevKitWallet::new(
            &seed,
            esplora,
            Network::Regtest,
            memory_storage.clone(),
            Some(Arc::new(DummyAddressGenerator)),
            logger.clone(),
        )
        .await
        .unwrap();

        let generate_address = wallet.get_new_address().await.unwrap();
        assert_eq!(generate_address, address);

        let generate_change_address = wallet.get_new_change_address().await.unwrap();
        assert_eq!(generate_change_address, change_address);

        let internal_wallet_address = wallet.new_external_address().await.unwrap();
        assert_ne!(internal_wallet_address.address, address);

        let internal_wallet_change_address = wallet.new_change_address().await.unwrap();
        assert_ne!(internal_wallet_change_address.address, change_address);

        let check_again = wallet.get_new_address().await.unwrap();
        assert_eq!(check_again, address);

        let check_again_change = wallet.get_new_change_address().await.unwrap();
        assert_eq!(check_again_change, change_address);
    }
}

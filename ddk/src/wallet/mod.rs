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

use crate::contract::ContractKeyProvider;
use crate::error::{wallet_err_to_manager_err, WalletError};
use crate::logger::Logger;
use crate::logger::{log_error, log_info, WriteLog};
use crate::wallet::address::AddressGenerator;
use crate::{chain::EsploraClient, Storage};
use bdk_chain::Balance;
use bdk_wallet::coin_selection::{
    BranchAndBoundCoinSelection, CoinSelectionAlgorithm, SingleRandomDraw,
};
use bdk_wallet::descriptor::IntoWalletDescriptor;
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
use bitcoin::bip32::Fingerprint;
use bitcoin::key::rand::thread_rng;
use bitcoin::Psbt;
use bitcoin::{secp256k1::SecretKey, Amount, FeeRate, ScriptBuf, Transaction};
use ddk_manager::{error::Error as ManagerError, SimpleSigner};
use lightning::chain::chaininterface::{ConfirmationTarget, FeeEstimator};
use std::collections::HashMap;
use std::fmt::Debug;
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::AtomicU32;
use std::sync::{atomic::Ordering, Arc};
use tokio::sync::{
    mpsc::{channel, Sender},
    oneshot,
};

type FutureResult<'a, T, E> = Pin<Box<dyn Future<Output = std::result::Result<T, E>> + Send + 'a>>;
type Result<T> = std::result::Result<T, WalletError>;

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
    /// Deterministic derivation of contract funding keys.
    contract_keys: ContractKeyProvider,
    /// Function to generate external addresses
    address_generator: Option<Arc<dyn AddressGenerator + Send + Sync>>,
    /// Logger
    logger: Arc<Logger>,
}

const MIN_FEERATE: u32 = 253;

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
            Bip84(xprv, KeychainKind::External).into_wallet_descriptor(&secp, network.into())?;
        let internal_descriptor =
            Bip84(xprv, KeychainKind::Internal).into_wallet_descriptor(&secp, network.into())?;

        let mut storage = WalletStorage(storage);

        let load_wallet = Wallet::load()
            .descriptor(KeychainKind::External, Some(external_descriptor.clone()))
            .descriptor(KeychainKind::Internal, Some(internal_descriptor.clone()))
            .extract_keys()
            .check_network(network)
            .load_wallet_async(&mut storage)
            .await
            .map_err(|e| WalletError::WalletPersistanceError(e.to_string()))?;

        let mut wallet = match load_wallet {
            Some(w) => w,
            None => Wallet::create(external_descriptor, internal_descriptor)
                .network(network)
                .create_wallet_async(&mut storage)
                .await
                .map_err(|e| WalletError::WalletPersistanceError(e.to_string()))?,
        };

        let contract_keys = ContractKeyProvider::from_xprv(xprv);

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
                        // A dropped persist error loses the revealed index and
                        // leads to address reuse after a restart.
                        let result = wallet
                            .persist_async(&mut storage)
                            .await
                            .map(|_| address)
                            .map_err(|e| WalletError::WalletPersistanceError(e.to_string()));
                        let _ = sender.send(result).map_err(|e| {
                            log_error!(
                                logger_clone,
                                "Error sending new external address command. error={:?}",
                                e
                            );
                        });
                    }
                    WalletCommand::NewChangeAddress(sender) => {
                        let address = wallet.next_unused_address(KeychainKind::Internal);
                        let result = wallet
                            .persist_async(&mut storage)
                            .await
                            .map(|_| address)
                            .map_err(|e| WalletError::WalletPersistanceError(e.to_string()));
                        let _ = sender.send(result).map_err(|e| {
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
            contract_keys,
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
    fn derive_signer_key_id(&self, is_offer_party: bool, temp_id: [u8; 32]) -> [u8; 32] {
        self.contract_keys
            .derive_signer_key_id(is_offer_party, temp_id)
    }

    /// Creates a contract signer from a key ID by delegating to the
    /// [`ContractKeyProvider`].
    #[tracing::instrument(skip(self, key_id))]
    fn derive_contract_signer(
        &self,
        key_id: [u8; 32],
    ) -> std::result::Result<Self::Signer, ManagerError> {
        self.contract_keys.derive_contract_signer(key_id)
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
                FeeRate::from_sat_per_vb(fee_rate).ok_or_else(|| {
                    ManagerError::WalletError(Box::new(WalletError::Esplora(format!(
                        "Invalid fee rate: {fee_rate}"
                    ))))
                })?,
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
    use std::{collections::HashSet, str::FromStr, sync::Arc};

    use crate::chain::EsploraClient;
    use crate::logger::{LogLevel, Logger};
    use crate::storage::memory::MemoryStorage;
    use bitcoin::{
        address::NetworkChecked, key::rand::Fill, Address, AddressType, Amount, FeeRate, Network,
    };
    use ddk_manager::{ContractSigner, ContractSignerProvider};

    use super::DlcDevKitWallet;

    async fn create_wallet_from_seed(seed: &[u8; 64]) -> DlcDevKitWallet {
        let esplora = ddk_testenv::env().esplora_host().to_string();
        create_wallet_on(&esplora, seed).await
    }

    async fn create_wallet_on(esplora: &str, seed: &[u8; 64]) -> DlcDevKitWallet {
        let storage = Arc::new(MemoryStorage::new());
        let logger = Arc::new(Logger::console(
            "console_logger".to_string(),
            LogLevel::Info,
        ));
        let esplora =
            Arc::new(EsploraClient::new(esplora, Network::Regtest, logger.clone()).unwrap());
        DlcDevKitWallet::new(
            seed,
            esplora,
            Network::Regtest,
            storage.clone(),
            None,
            logger.clone(),
        )
        .await
        .unwrap()
    }

    async fn create_wallet() -> DlcDevKitWallet {
        let mut entropy = [0u8; 64];
        entropy
            .try_fill(&mut bitcoin::key::rand::thread_rng())
            .unwrap();
        create_wallet_from_seed(&entropy).await
    }

    fn generate_blocks(num: u64) {
        ddk_testenv::env().generate_blocks(num);
    }

    fn fund_address(address: &Address<NetworkChecked>) {
        ddk_testenv::env().fund_address(address, Amount::from_btc(1.0).unwrap());
        generate_blocks(4)
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
    async fn sees_mempool_transaction_without_new_block() {
        // A private environment: a block mined by a concurrent test would
        // change the chain height and confirm the transaction.
        let env = ddk_testenv::TestEnv::new();
        let mut seed = [0u8; 64];
        seed.try_fill(&mut bitcoin::key::rand::thread_rng())
            .unwrap();
        let wallet = create_wallet_on(env.esplora_host(), &seed).await;
        let address = wallet.new_external_address().await.unwrap().address;
        // Bring the wallet tip up to the current chain height first.
        wallet.sync().await.unwrap();

        // Send to the address without mining a block: the transaction stays
        // in the mempool and the chain height does not change.
        let txid = env.send_to_address(&address, Amount::from_btc(0.5).unwrap());
        env.wait_for_tx(&txid);

        wallet.sync().await.unwrap();
        let pending = wallet.get_balance().await.unwrap().untrusted_pending;
        assert!(pending > Amount::ZERO);
    }

    #[tokio::test]
    async fn drops_replaced_mempool_transaction() {
        use bitcoincore_rpc::RpcApi;
        use std::collections::HashMap;

        // A private environment: a block mined by a concurrent test would
        // confirm the transaction before it can be replaced.
        let env = ddk_testenv::TestEnv::new();
        let rpc = env.rpc();

        let mut seed = [0u8; 64];
        seed.try_fill(&mut bitcoin::key::rand::thread_rng())
            .unwrap();
        let wallet = create_wallet_on(env.esplora_host(), &seed).await;
        let address = wallet.new_external_address().await.unwrap().address;
        wallet.sync().await.unwrap();

        // Build an RBF transaction that pays the wallet.
        let unspent = rpc
            .list_unspent(Some(1), None, None, None, None)
            .unwrap()
            .into_iter()
            .find(|utxo| utxo.amount >= Amount::from_btc(1.0).unwrap() && utxo.spendable)
            .unwrap();
        let input = bitcoincore_rpc::json::CreateRawTransactionInput {
            txid: unspent.txid,
            vout: unspent.vout,
            sequence: None,
        };
        let change = rpc.get_new_address(None, None).unwrap().assume_checked();
        let payment = Amount::from_btc(0.5).unwrap();
        let fee = Amount::from_sat(2_000);
        let mut outputs = HashMap::new();
        outputs.insert(address.to_string(), payment);
        outputs.insert(change.to_string(), unspent.amount - payment - fee);
        let raw = rpc
            .create_raw_transaction(&[input.clone()], &outputs, None, Some(true))
            .unwrap();
        let signed = rpc
            .sign_raw_transaction_with_wallet(&raw, None, None)
            .unwrap();
        let txid = rpc.send_raw_transaction(&signed.hex).unwrap();
        env.wait_for_tx(&txid);

        wallet.sync().await.unwrap();
        assert_eq!(
            wallet.get_balance().await.unwrap().untrusted_pending,
            payment
        );

        // Replace it with a conflicting transaction that does not pay the
        // wallet. The wallet only learns of the eviction from esplora.
        let replacement_fee = Amount::from_sat(50_000);
        let mut replacement_outputs = HashMap::new();
        replacement_outputs.insert(change.to_string(), unspent.amount - replacement_fee);
        let raw = rpc
            .create_raw_transaction(&[input], &replacement_outputs, None, Some(true))
            .unwrap();
        let signed = rpc
            .sign_raw_transaction_with_wallet(&raw, None, None)
            .unwrap();
        let replacement_txid = rpc.send_raw_transaction(&signed.hex).unwrap();
        env.wait_for_tx(&replacement_txid);

        wallet.sync().await.unwrap();
        assert_eq!(
            wallet.get_balance().await.unwrap().untrusted_pending,
            Amount::ZERO
        );
    }

    #[tokio::test]
    async fn restore_from_seed_finds_change_outputs() {
        let mut seed = [0u8; 64];
        seed.try_fill(&mut bitcoin::key::rand::thread_rng())
            .unwrap();

        let original = create_wallet_from_seed(&seed).await;
        let addr = original.new_external_address().await.unwrap().address;
        fund_address(&addr);
        original.sync().await.unwrap();

        // Spend to a foreign address so the only remaining funds sit on an
        // internal (change) output.
        let dest = Address::from_str("bcrt1qt0yrvs7qx8guvpqsx8u9mypz6t4zr3pxthsjkm")
            .unwrap()
            .assume_checked();
        original
            .send_to_address(
                dest,
                Amount::from_sat(10_000_000),
                FeeRate::from_sat_per_vb(1).unwrap(),
            )
            .await
            .unwrap();
        generate_blocks(1);
        original.sync().await.unwrap();
        let original_balance = original.get_balance().await.unwrap();
        assert!(original_balance.confirmed > Amount::ZERO);

        // Restore from the same seed into empty storage. The full scan must
        // discover the used change address on the internal keychain.
        let restored = create_wallet_from_seed(&seed).await;
        restored.sync().await.unwrap();
        let restored_balance = restored.get_balance().await.unwrap();
        assert_eq!(restored_balance.confirmed, original_balance.confirmed);
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
        let esplora_host = ddk_testenv::env().esplora_host();
        let esplora =
            Arc::new(EsploraClient::new(esplora_host, Network::Regtest, logger.clone()).unwrap());

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

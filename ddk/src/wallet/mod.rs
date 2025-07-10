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

mod command;

use crate::error::{wallet_err_to_manager_err, WalletError};
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
use bitcoin::hashes::sha256::Hash as Sha256Hash;
use bitcoin::hashes::sha256::HashEngine;
use bitcoin::hashes::Hash;
use bitcoin::key::rand::{thread_rng, Fill};
use bitcoin::Psbt;
use bitcoin::{secp256k1::SecretKey, Amount, FeeRate, ScriptBuf, Transaction};
use ddk_manager::{error::Error as ManagerError, SimpleSigner};
use lightning::chain::chaininterface::{ConfirmationTarget, FeeEstimator};
use std::collections::HashMap;
use std::future::Future;
use std::io::Write;
use std::pin::Pin;
use std::sync::atomic::AtomicU32;
use std::sync::{atomic::Ordering, Arc};
use tokio::sync::{
    mpsc::{channel, Sender},
    oneshot,
};

type FutureResult<'a, T, E> = Pin<Box<dyn Future<Output = std::result::Result<T, E>> + Send + 'a>>;
type Result<T> = std::result::Result<T, WalletError>;

/// Wrapper type that adapts DDK's Storage trait to BDK's AsyncWalletPersister interface.
///
/// This wrapper is necessary because BDK requires a persister that implements AsyncWalletPersister,
/// but DDK's Storage trait provides a different interface. The wrapper provides thread safety
/// and interior mutability required by BDK while delegating to the underlying DDK storage.
///
/// # Thread Safety
/// The wrapper uses Arc<dyn Storage> to ensure the storage can be safely shared across threads
/// and provides the necessary interior mutability for BDK operations.
#[derive(Clone)]
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
        tracing::info!("persist store");
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
    pub async fn new(
        seed_bytes: &[u8; 32],
        esplora_url: &str,
        network: Network,
        storage: Arc<dyn Storage>,
    ) -> Result<DlcDevKitWallet> {
        let secp = Secp256k1::new();

        let xprv = Xpriv::new_master(network, seed_bytes)?;

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
            .map_err(|e| WalletError::WalletPersistanceError(e.to_string()))?;

        let mut wallet = match load_wallet {
            Some(w) => w,
            None => Wallet::create(external_descriptor, internal_descriptor)
                .network(network)
                .create_wallet_async(&mut storage)
                .await
                .map_err(|e| WalletError::WalletPersistanceError(e.to_string()))?,
        };

        let blockchain = Arc::new(
            EsploraClient::new(esplora_url, network)
                .map_err(|e| WalletError::Esplora(e.to_string()))?,
        );

        let (sender, mut receiver) = channel(100);

        tokio::spawn(async move {
            while let Some(command) = receiver.recv().await {
                match command {
                    WalletCommand::Sync(sender) => {
                        let sync = command::sync(&mut wallet, &blockchain, &mut storage).await;
                        let _ = sender.send(sync).map_err(|e| {
                            tracing::error!("Error sending sync command: {:?}", e);
                        });
                    }
                    WalletCommand::Balance(sender) => {
                        let balance = wallet.balance();
                        let _ = sender.send(balance).map_err(|e| {
                            tracing::error!("Error sending balance command: {:?}", e);
                        });
                    }
                    WalletCommand::NewExternalAddress(sender) => {
                        let address = wallet.next_unused_address(KeychainKind::External);
                        let _ = wallet.persist_async(&mut storage).await;
                        let _ = sender.send(Ok(address)).map_err(|e| {
                            tracing::error!("Error sending new external address command: {:?}", e);
                        });
                    }
                    WalletCommand::NewChangeAddress(sender) => {
                        let address = wallet.next_unused_address(KeychainKind::Internal);
                        let _ = wallet.persist_async(&mut storage).await;
                        let _ = sender.send(Ok(address)).map_err(|e| {
                            tracing::error!("Error sending new change address command: {:?}", e);
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
                                    tracing::error!(
                                        "Error sending send to address command: {:?}",
                                        e
                                    );
                                });
                                continue;
                            }
                        };
                        if let Err(e) = wallet.sign(&mut psbt, SignOptions::default()) {
                            let _ = sender.send(Err(WalletError::Signing(e))).map_err(|e| {
                                tracing::error!("Error sending send to address command: {:?}", e);
                            });
                            continue;
                        }
                        let tx = match psbt.extract_tx() {
                            Ok(tx) => tx,
                            Err(_) => {
                                let _ = sender.send(Err(WalletError::ExtractTx)).map_err(|e| {
                                    tracing::error!(
                                        "Error sending send to address command: {:?}",
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
                                    tracing::error!(
                                        "Error sending send to address command: {:?}",
                                        e
                                    );
                                });
                            continue;
                        }
                        let _ = sender.send(Ok(txid)).map_err(|e| {
                            tracing::error!("Error sending send to address command: {:?}", e);
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
                                    tracing::error!("Error sending send all command: {:?}", e);
                                });
                                continue;
                            }
                        };
                        if let Err(e) = wallet.sign(&mut psbt, SignOptions::default()) {
                            let _ = sender.send(Err(WalletError::Signing(e))).map_err(|e| {
                                tracing::error!("Error sending send all command: {:?}", e);
                            });
                            continue;
                        }
                        let tx = match psbt.extract_tx() {
                            Ok(tx) => tx,
                            Err(_) => {
                                let _ = sender.send(Err(WalletError::ExtractTx)).map_err(|e| {
                                    tracing::error!("Error sending send all command: {:?}", e);
                                });
                                continue;
                            }
                        };
                        let txid = tx.compute_txid();
                        if let Err(e) = blockchain.async_client.broadcast(&tx).await {
                            let _ = sender
                                .send(Err(WalletError::Esplora(e.to_string())))
                                .map_err(|e| {
                                    tracing::error!("Error sending send all command: {:?}", e);
                                });
                            continue;
                        }
                        let _ = sender.send(Ok(txid)).map_err(|e| {
                            tracing::error!("Error sending send all command: {:?}", e);
                        });
                    }
                    WalletCommand::GetTransactions(sender) => {
                        let txs = wallet
                            .transactions()
                            .map(|t| t.tx_node.tx)
                            .collect::<Vec<Arc<Transaction>>>();
                        let _ = sender.send(Ok(txs)).map_err(|e| {
                            tracing::error!("Error sending get transactions command: {:?}", e);
                        });
                    }
                    WalletCommand::ListUtxos(sender) => {
                        let utxos = wallet.list_unspent().map(|utxo| utxo.to_owned()).collect();
                        let _ = sender.send(Ok(utxos)).map_err(|e| {
                            tracing::error!("Error sending list utxos command: {:?}", e);
                        });
                    }
                    WalletCommand::NextDerivationIndex(sender) => {
                        let index = wallet.next_derivation_index(KeychainKind::External);
                        let _ = sender.send(Ok(index)).map_err(|e| {
                            tracing::error!("Error sending next derivation index command: {:?}", e);
                        });
                    }
                    WalletCommand::SignPsbtInput(mut psbt, input_index, sender) => {
                        let sign_opts = SignOptions {
                            trust_witness_utxo: true,
                            ..Default::default()
                        };
                        let mut signed_psbt = psbt.clone();
                        if let Err(e) = wallet.sign(&mut signed_psbt, sign_opts) {
                            tracing::error!("Could not sign PSBT: {:?}", e);
                            let _ = sender
                                .send(Err(ManagerError::WalletError(
                                    WalletError::Signing(e).into(),
                                )))
                                .map_err(|e| {
                                    tracing::error!(
                                        "Error sending sign psbt input command: {:?}",
                                        e
                                    );
                                });
                        } else {
                            psbt.inputs[input_index] = signed_psbt.inputs[input_index].clone();
                            let _ = sender.send(Ok(psbt)).map_err(|e| {
                                tracing::error!("Error sending sign psbt input command: {:?}", e);
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
        })
    }

    /// Synchronizes the wallet with the blockchain.
    /// This updates the wallet's UTXO set and transaction history.
    pub async fn sync(&self) -> Result<()> {
        let (tx, rx) = oneshot::channel();
        self.sender.send(WalletCommand::Sync(tx)).await?;
        rx.await.map_err(WalletError::Receiver)?
    }

    /// Returns the wallet's master public key.
    /// Used for identification and key derivation.
    pub fn get_pubkey(&self) -> PublicKey {
        tracing::info!("Getting wallet public key.");
        PublicKey::from_secret_key(&self.secp, &self.xprv.private_key)
    }

    /// Retrieves the current wallet balance including confirmed and unconfirmed amounts.
    pub async fn get_balance(&self) -> Result<Balance> {
        let (tx, rx) = oneshot::channel();
        self.sender.send(WalletCommand::Balance(tx)).await?;
        rx.await.map_err(WalletError::Receiver)
    }

    /// Generates a new external (receiving) address.
    /// These addresses are used for receiving funds from external sources.
    pub async fn new_external_address(&self) -> Result<AddressInfo> {
        let (tx, rx) = oneshot::channel();
        self.sender
            .send(WalletCommand::NewExternalAddress(tx))
            .await?;
        rx.await.map_err(WalletError::Receiver)?
    }

    /// Generates a new change address.
    /// These addresses are used internally for change outputs.
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
    pub async fn send_all(&self, address: Address, fee_rate: FeeRate) -> Result<Txid> {
        let (tx, rx) = oneshot::channel();
        self.sender
            .send(WalletCommand::SendAll(address, fee_rate, tx))
            .await?;
        rx.await.map_err(WalletError::Receiver)?
    }

    /// Retrieves all transactions known to the wallet.
    pub async fn get_transactions(&self) -> Result<Vec<Arc<Transaction>>> {
        let (tx, rx) = oneshot::channel();
        self.sender.send(WalletCommand::GetTransactions(tx)).await?;
        rx.await.map_err(WalletError::Receiver)?
    }

    /// Lists all unspent transaction outputs (UTXOs) in the wallet.
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
    async fn sign_psbt_input(
        &self,
        psbt: &mut bitcoin::psbt::Psbt,
        input_index: usize,
    ) -> std::result::Result<(), ManagerError> {
        tracing::info!(
            input_index,
            inputs = psbt.inputs.len(),
            outputs = psbt.outputs.len(),
            "Signing psbt input for dlc manager."
        );
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
    fn derive_signer_key_id(&self, _is_offer_party: bool, temp_id: [u8; 32]) -> [u8; 32] {
        let mut random_bytes = [0u8; 32];
        let _ = random_bytes.try_fill(&mut thread_rng()).map_err(|e| {
            tracing::error!(
                "Did not create random bytes while generating key id. {:?}",
                e
            );
        });
        let mut hasher = HashEngine::default();
        hasher.write_all(&temp_id).unwrap();
        hasher.write_all(&random_bytes).unwrap();
        let hash: Sha256Hash = Hash::from_engine(hasher);

        // Might want to store this for safe backups.
        hash.to_byte_array()
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
    fn derive_contract_signer(
        &self,
        key_id: [u8; 32],
    ) -> std::result::Result<Self::Signer, ManagerError> {
        let child_key = SecretKey::from_slice(&key_id).expect("correct size");
        tracing::info!(
            key_id = hex::encode(key_id),
            "Derived secret key for contract."
        );
        Ok(SimpleSigner::new(child_key))
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
        let address = self
            .new_external_address()
            .await
            .map_err(wallet_err_to_manager_err)?;
        tracing::info!(
            address = address.address.to_string(),
            "Revealed new address for contract."
        );
        Ok(address.address)
    }

    /// Gets a new change address for transaction outputs.
    /// Used by the DLC manager for change outputs in DLC transactions.
    async fn get_new_change_address(&self) -> std::result::Result<bitcoin::Address, ManagerError> {
        let address = self
            .new_change_address()
            .await
            .map_err(wallet_err_to_manager_err)?;

        tracing::info!(
            address = address.address.to_string(),
            "Revealed new change address for contract."
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
        tracing::info!(
            input_index,
            inputs = psbt.inputs.len(),
            outputs = psbt.outputs.len(),
            "Signing psbt input for dlc manager."
        );
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

        let selected_utxos =
            BranchAndBoundCoinSelection::new(Amount::MAX_MONEY.to_sat(), SingleRandomDraw)
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

#[cfg(test)]
mod tests {
    use std::{str::FromStr, sync::Arc, time::Duration};

    use crate::storage::memory::MemoryStorage;
    use bitcoin::{
        address::NetworkChecked, bip32::Xpriv, key::rand::Fill, Address, AddressType, Amount,
        FeeRate, Network,
    };
    use bitcoincore_rpc::RpcApi;
    use ddk_manager::ContractSignerProvider;

    use super::DlcDevKitWallet;

    async fn create_wallet() -> DlcDevKitWallet {
        let esplora = std::env::var("ESPLORA_HOST").unwrap_or("http://localhost:30000".to_string());
        let storage = Arc::new(MemoryStorage::new());
        let mut entropy = [0u8; 64];
        entropy
            .try_fill(&mut bitcoin::key::rand::thread_rng())
            .unwrap();
        let xpriv = Xpriv::new_master(Network::Regtest, &entropy).unwrap();
        DlcDevKitWallet::new(
            &xpriv.private_key.secret_bytes(),
            &esplora,
            Network::Regtest,
            storage.clone(),
        )
        .await
        .unwrap()
    }

    fn generate_blocks(num: u64) {
        tracing::warn!("Generating {} blocks.", num);
        let bitcoind =
            std::env::var("BITCOIND_HOST").unwrap_or("http://localhost:18443".to_string());
        let auth = bitcoincore_rpc::Auth::UserPass("ddk".to_string(), "ddk".to_string());
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
        let auth = bitcoincore_rpc::Auth::UserPass("ddk".to_string(), "ddk".to_string());
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
}

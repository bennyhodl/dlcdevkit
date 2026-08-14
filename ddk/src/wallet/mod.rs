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
pub mod contract_tracker;

pub use command::{CoinControl, Spend};

use crate::contract::ContractKeyProvider;
use crate::error::{wallet_err_to_manager_err, WalletError};
use crate::logger::Logger;
use crate::logger::{log_error, log_info, WriteLog};
use crate::wallet::address::AddressGenerator;
use crate::{chain::EsploraClient, Storage};
use bdk_wallet::descriptor::IntoWalletDescriptor;
use bdk_wallet::AsyncWalletPersister;
pub use bdk_wallet::LocalOutput;
pub use bdk_wallet::WalletEvent;
use bdk_wallet::{
    bitcoin::{
        bip32::Xpriv,
        secp256k1::{All, PublicKey, Secp256k1},
        Address, Network, Txid,
    },
    template::Bip84,
    AddressInfo, KeychainKind, SignOptions, Wallet,
};
use bitcoin::bip32::Fingerprint;
use bitcoin::Psbt;
use bitcoin::{secp256k1::SecretKey, Amount, FeeRate, Transaction};
use ddk_manager::{error::Error as ManagerError, SimpleSigner};
use lightning::chain::chaininterface::{ConfirmationTarget, FeeEstimator};
use std::fmt::Debug;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use tokio::sync::{
    mpsc::{channel, Sender},
    oneshot,
};

type FutureResult<'a, T, E> = Pin<Box<dyn Future<Output = std::result::Result<T, E>> + Send + 'a>>;
type Result<T> = std::result::Result<T, WalletError>;

/// The minimum change size for the wallet to create in coin selection.
const MIN_CHANGE_SIZE: u64 = 25_000;

/// The wallet balance, split into the BDK balance categories plus the
/// spendable/reserved view over the outpoint locks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct WalletBalance {
    /// Confirmed and mature coins
    pub confirmed: Amount,
    /// Unconfirmed coins from the wallet's own transactions
    pub trusted_pending: Amount,
    /// Unconfirmed coins from foreign transactions
    pub untrusted_pending: Amount,
    /// Immature coinbase outputs
    pub immature: Amount,
    /// Unspent coins minus reserved coins: what a new selection can spend
    pub spendable: Amount,
    /// Unspent coins locked for in-flight offers and funding transactions
    pub reserved: Amount,
    /// Confirmed contract funding outputs, from chain truth
    pub contract_confirmed: Amount,
    /// Unconfirmed contract funding outputs, from chain truth
    pub contract_pending: Amount,
}

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
    Balance(oneshot::Sender<WalletBalance>),

    /// Generate a new external (receiving) address
    NewExternalAddress(oneshot::Sender<Result<AddressInfo>>),

    /// Generate a new internal (change) address
    NewChangeAddress(oneshot::Sender<Result<AddressInfo>>),

    /// Send to an address: an amount or the whole wallet, with optional
    /// coin control
    Send {
        /// Destination address
        address: Address,
        /// What the send spends
        spend: command::Spend,
        /// Fee rate of the transaction
        fee_rate: FeeRate,
        /// Restrictions on which UTXOs fund the transaction
        coin_control: command::CoinControl,
        /// Channel for receiving the transaction id
        responder: oneshot::Sender<Result<Txid>>,
    },

    /// Replace an unconfirmed wallet transaction with a higher-fee version
    BumpFee {
        /// The transaction to replace
        txid: Txid,
        /// The fee rate of the replacement; must beat the original's
        fee_rate: FeeRate,
        /// Channel for receiving the replacement transaction id
        responder: oneshot::Sender<Result<Txid>>,
    },

    /// Get all wallet transactions
    GetTransactions(oneshot::Sender<Result<Vec<Arc<Transaction>>>>),

    /// List all unspent transaction outputs (UTXOs)
    ListUtxos(oneshot::Sender<Result<Vec<LocalOutput>>>),

    /// Get the next derivation index for address generation
    NextDerivationIndex(oneshot::Sender<Result<u32>>),

    /// Sign every wallet-owned input of a PSBT (Partially Signed Bitcoin Transaction)
    SignPsbt(
        bitcoin::psbt::Psbt,
        oneshot::Sender<std::result::Result<Psbt, ManagerError>>,
    ),

    /// Select UTXOs that cover an amount, optionally locking them
    SelectUtxos {
        /// The amount the selection must cover
        amount: Amount,
        /// Fee rate in sat/vB the selection pays for its own weight
        fee_rate: u64,
        /// Lock the selected outpoints against concurrent selections
        lock_utxos: bool,
        /// Channel for receiving the selected UTXOs
        responder: oneshot::Sender<Result<Vec<ddk_manager::Utxo>>>,
    },

    /// Unlock previously locked outpoints and persist the change
    UnlockOutpoints(Vec<bitcoin::OutPoint>, oneshot::Sender<Result<()>>),

    /// List the currently locked outpoints
    ListLockedOutpoints(oneshot::Sender<Vec<bitcoin::OutPoint>>),

    /// List the tracked contract funding outputs with their chain state
    ContractUtxos(oneshot::Sender<Vec<contract_tracker::ContractUtxo>>),
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
    /// Broadcast sender for wallet events; new subscribers come from
    /// [`DlcDevKitWallet::subscribe_events`]
    events: tokio::sync::broadcast::Sender<WalletEvent>,
    /// Esplora client, the source of the cached live fee estimates
    blockchain: Arc<EsploraClient>,
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
        let (events, _) = tokio::sync::broadcast::channel(256);
        let blockchain_handle = blockchain.clone();

        let mut tracker = contract_tracker::ContractUtxoTracker::from_changeset(
            storage.0.initialize_contract_tracker().await?,
        );

        let events_clone = events.clone();
        let logger_clone = logger.clone();
        tokio::spawn(async move {
            while let Some(command) = receiver.recv().await {
                match command {
                    WalletCommand::Sync(sender) => {
                        let sync = command::sync(
                            &mut wallet,
                            &mut tracker,
                            &blockchain,
                            &mut storage,
                            &events_clone,
                            logger_clone.clone(),
                        )
                        .await;
                        let _ = sender.send(sync).map_err(|e| {
                            log_error!(logger_clone, "Error sending sync command. error={:?}", e);
                        });
                    }
                    WalletCommand::Balance(sender) => {
                        let balance = wallet.balance();
                        let reserved = wallet
                            .list_unspent()
                            .filter(|utxo| wallet.is_outpoint_locked(utxo.outpoint))
                            .map(|utxo| utxo.txout.value)
                            .sum::<Amount>();
                        let spendable = balance
                            .total()
                            .checked_sub(reserved)
                            .unwrap_or(Amount::ZERO);
                        let contract_balance = tracker.balance(wallet.local_chain());
                        let balance = WalletBalance {
                            confirmed: balance.confirmed,
                            trusted_pending: balance.trusted_pending,
                            untrusted_pending: balance.untrusted_pending,
                            immature: balance.immature,
                            spendable,
                            reserved,
                            contract_confirmed: contract_balance.confirmed,
                            contract_pending: contract_balance.trusted_pending
                                + contract_balance.untrusted_pending,
                        };
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
                    WalletCommand::Send {
                        address,
                        spend,
                        fee_rate,
                        coin_control,
                        responder,
                    } => {
                        let result = command::send(
                            &mut wallet,
                            &blockchain,
                            &mut storage,
                            address,
                            spend,
                            fee_rate,
                            coin_control,
                        )
                        .await;
                        let _ = responder.send(result).map_err(|e| {
                            log_error!(logger_clone, "Error sending send command. error={:?}", e);
                        });
                    }
                    WalletCommand::BumpFee {
                        txid,
                        fee_rate,
                        responder,
                    } => {
                        let result = command::bump_fee(
                            &mut wallet,
                            &blockchain,
                            &mut storage,
                            txid,
                            fee_rate,
                        )
                        .await;
                        let _ = responder.send(result).map_err(|e| {
                            log_error!(
                                logger_clone,
                                "Error sending bump fee command. error={:?}",
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
                    WalletCommand::SignPsbt(mut psbt, sender) => {
                        let sign_opts = SignOptions {
                            trust_witness_utxo: true,
                            ..Default::default()
                        };
                        // Sign in place, once, for every input the wallet
                        // owns. A caller that walks a funding transaction
                        // input-by-input finds later inputs already
                        // finalized, so the signing work is done one time.
                        let result = match wallet.sign(&mut psbt, sign_opts) {
                            Ok(_) => {
                                // A signed funding transaction can stay
                                // unbroadcast while the counterparty
                                // finishes the protocol. Lock its wallet
                                // inputs so a concurrent selection cannot
                                // double-spend them; the locks release when
                                // the spend confirms.
                                let wallet_inputs = psbt
                                    .unsigned_tx
                                    .input
                                    .iter()
                                    .map(|input| input.previous_output)
                                    .filter(|outpoint| wallet.get_utxo(*outpoint).is_some())
                                    .collect::<Vec<_>>();
                                for outpoint in wallet_inputs {
                                    wallet.lock_outpoint(outpoint);
                                }
                                if let Err(e) = wallet.persist_async(&mut storage).await {
                                    log_error!(
                                        logger_clone,
                                        "Could not persist locks for signed funding inputs. error={:?}",
                                        e
                                    );
                                }
                                Ok(psbt)
                            }
                            Err(e) => {
                                log_error!(logger_clone, "Could not sign PSBT. error={:?}", e);
                                Err(ManagerError::WalletError(WalletError::Signing(e).into()))
                            }
                        };
                        let _ = sender.send(result).map_err(|e| {
                            log_error!(
                                logger_clone,
                                "Error sending sign psbt command. error={:?}",
                                e
                            );
                        });
                    }
                    WalletCommand::SelectUtxos {
                        amount,
                        fee_rate,
                        lock_utxos,
                        responder,
                    } => {
                        let result = command::select_utxos(
                            &mut wallet,
                            &mut storage,
                            amount,
                            fee_rate,
                            lock_utxos,
                        )
                        .await;
                        let _ = responder.send(result).map_err(|e| {
                            log_error!(
                                logger_clone,
                                "Error sending select utxos command. error={:?}",
                                e
                            );
                        });
                    }
                    WalletCommand::UnlockOutpoints(outpoints, responder) => {
                        for outpoint in outpoints {
                            wallet.unlock_outpoint(outpoint);
                        }
                        let result = wallet
                            .persist_async(&mut storage)
                            .await
                            .map(|_| ())
                            .map_err(|e| WalletError::WalletPersistanceError(e.to_string()));
                        if let Err(e) = &result {
                            log_error!(
                                logger_clone,
                                "Could not persist unlocked outpoints. error={:?}",
                                e
                            );
                        }
                        // The responder is already dropped when the manager
                        // fires this command without waiting.
                        let _ = responder.send(result);
                    }
                    WalletCommand::ListLockedOutpoints(responder) => {
                        let outpoints = wallet.list_locked_outpoints().collect();
                        let _ = responder.send(outpoints).map_err(|e| {
                            log_error!(
                                logger_clone,
                                "Error sending list locked outpoints command. error={:?}",
                                e
                            );
                        });
                    }
                    WalletCommand::ContractUtxos(responder) => {
                        let utxos = tracker.utxos(wallet.local_chain());
                        let _ = responder.send(utxos).map_err(|e| {
                            log_error!(
                                logger_clone,
                                "Error sending contract utxos command. error={:?}",
                                e
                            );
                        });
                    }
                }
            }
        });

        Ok(DlcDevKitWallet {
            sender,
            events,
            blockchain: blockchain_handle,
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

    /// Subscribes to wallet events. Each sync emits an event for every
    /// transaction that confirmed, unconfirmed, was replaced, or was
    /// dropped, with reorg awareness from BDK.
    pub fn subscribe_events(&self) -> tokio::sync::broadcast::Receiver<WalletEvent> {
        self.events.subscribe()
    }

    /// Returns the wallet's master public key.
    /// Used for identification and key derivation.
    #[tracing::instrument(skip(self))]
    pub fn get_pubkey(&self) -> PublicKey {
        PublicKey::from_secret_key(&self.secp, &self.xprv.private_key)
    }

    /// Retrieves the current wallet balance including confirmed and
    /// unconfirmed amounts, plus the spendable/reserved split over the
    /// outpoint locks.
    #[tracing::instrument(skip(self))]
    pub async fn get_balance(&self) -> Result<WalletBalance> {
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
        self.send_with_coin_control(
            address,
            Spend::Amount(amount),
            fee_rate,
            CoinControl::default(),
        )
        .await
    }

    /// Sends with coin control: caller-selected UTXOs, an unspendable
    /// exclusion list, and a confirmation floor.
    ///
    /// # Arguments
    /// * `address` - Destination Bitcoin address
    /// * `spend` - An amount, or the whole wallet
    /// * `fee_rate` - Fee rate for the transaction
    /// * `coin_control` - Restrictions on which UTXOs fund the transaction
    ///
    /// # Returns
    /// Transaction ID of the sent transaction
    #[tracing::instrument(skip(self))]
    pub async fn send_with_coin_control(
        &self,
        address: Address,
        spend: Spend,
        fee_rate: FeeRate,
        coin_control: CoinControl,
    ) -> Result<Txid> {
        let (tx, rx) = oneshot::channel();
        self.sender
            .send(WalletCommand::Send {
                address,
                spend,
                fee_rate,
                coin_control,
                responder: tx,
            })
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
        self.send_with_coin_control(address, Spend::All, fee_rate, CoinControl::default())
            .await
    }

    /// Replaces a stuck unconfirmed wallet transaction with a
    /// higher-fee version (RBF is on by default for wallet
    /// transactions).
    ///
    /// # Arguments
    /// * `txid` - The transaction to replace; it must be unconfirmed
    /// * `fee_rate` - Fee rate of the replacement; must beat the original's
    ///
    /// # Returns
    /// Transaction ID of the replacement transaction
    #[tracing::instrument(skip(self))]
    pub async fn bump_fee(&self, txid: Txid, fee_rate: FeeRate) -> Result<Txid> {
        let (tx, rx) = oneshot::channel();
        self.sender
            .send(WalletCommand::BumpFee {
                txid,
                fee_rate,
                responder: tx,
            })
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

    /// Unlocks previously locked outpoints and waits until the change is
    /// persisted.
    #[tracing::instrument(skip(self))]
    pub async fn unlock_outpoints(&self, outpoints: Vec<bitcoin::OutPoint>) -> Result<()> {
        let (tx, rx) = oneshot::channel();
        self.sender
            .send(WalletCommand::UnlockOutpoints(outpoints, tx))
            .await?;
        rx.await.map_err(WalletError::Receiver)?
    }

    /// Lists the outpoints that are locked against coin selection.
    #[tracing::instrument(skip(self))]
    pub async fn locked_outpoints(&self) -> Result<Vec<bitcoin::OutPoint>> {
        let (tx, rx) = oneshot::channel();
        self.sender
            .send(WalletCommand::ListLockedOutpoints(tx))
            .await?;
        rx.await.map_err(WalletError::Receiver)
    }

    /// Lists the tracked contract funding outputs with their chain state,
    /// so a consumer can show locked collateral per contract and observe
    /// a close.
    #[tracing::instrument(skip(self))]
    pub async fn contract_utxos(&self) -> Result<Vec<contract_tracker::ContractUtxo>> {
        let (tx, rx) = oneshot::channel();
        self.sender.send(WalletCommand::ContractUtxos(tx)).await?;
        rx.await.map_err(WalletError::Receiver)
    }

    /// Signs a specific input in a PSBT for DLC operations.
    ///
    /// This method is used internally by the DLC manager to sign
    /// DLC-related transactions such as funding transactions. The manager
    /// calls it once per input; the wallet signs the full PSBT on the first
    /// call and later calls find their input already finalized.
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
        if psbt
            .inputs
            .get(input_index)
            .is_some_and(|input| input.final_script_witness.is_some())
        {
            return Ok(());
        }
        let (tx, rx) = oneshot::channel();
        self.sender
            .send(WalletCommand::SignPsbt(psbt.clone(), tx))
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
    /// Returns the estimated fee rate in satoshis per 1000 weight units,
    /// from the fee cache the esplora client refreshes on each sync.
    #[tracing::instrument(skip(self))]
    fn get_est_sat_per_1000_weight(&self, confirmation_target: ConfirmationTarget) -> u32 {
        self.blockchain
            .get_est_sat_per_1000_weight(confirmation_target)
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

    /// Unreserves UTXOs that were previously locked by coin selection.
    /// The manager calls this when an offer fails or a contract is
    /// rejected. The unlock command is fired without waiting because this
    /// trait method is synchronous; the actor persists the change.
    fn unreserve_utxos(
        &self,
        outpoints: &[bitcoin::OutPoint],
    ) -> std::result::Result<(), ManagerError> {
        let (tx, _rx) = oneshot::channel();
        self.sender
            .try_send(WalletCommand::UnlockOutpoints(outpoints.to_vec(), tx))
            .map_err(|e| wallet_err_to_manager_err(WalletError::SendMessage(e.to_string())))
    }

    /// Imports an address into the wallet for monitoring.
    /// Currently a no-op as address import is not needed.
    fn import_address(&self, _address: &bitcoin::Address) -> std::result::Result<(), ManagerError> {
        Ok(())
    }

    /// Selects UTXOs for a specific amount and fee rate.
    ///
    /// This method is used by the DLC manager to select appropriate UTXOs
    /// for funding DLC transactions. Locked outpoints are never selected.
    /// When `lock_utxos` is set, the selected outpoints are locked and the
    /// locks persist across restarts, so two concurrent offers cannot fund
    /// themselves with the same coins.
    ///
    /// # Arguments
    /// * `amount` - The amount of Bitcoin needed
    /// * `fee_rate` - The fee rate for the transaction
    /// * `lock_utxos` - Whether to lock the selected UTXOs
    ///
    /// # Returns
    /// A vector of UTXOs that can cover the required amount plus fees
    #[tracing::instrument(skip(self))]
    async fn get_utxos_for_amount(
        &self,
        amount: Amount,
        fee_rate: u64,
        lock_utxos: bool,
    ) -> std::result::Result<Vec<ddk_manager::Utxo>, ManagerError> {
        let (tx, rx) = oneshot::channel();
        self.sender
            .send(WalletCommand::SelectUtxos {
                amount,
                fee_rate,
                lock_utxos,
                responder: tx,
            })
            .await
            .map_err(|e| wallet_err_to_manager_err(WalletError::Sender(e)))?;
        rx.await
            .map_err(|e| wallet_err_to_manager_err(WalletError::Receiver(e)))?
            .map_err(wallet_err_to_manager_err)
    }
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
        create_wallet_with_storage(esplora, seed, Arc::new(MemoryStorage::new())).await
    }

    async fn create_wallet_with_storage(
        esplora: &str,
        seed: &[u8; 64],
        storage: Arc<MemoryStorage>,
    ) -> DlcDevKitWallet {
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
            storage,
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
    async fn concurrent_selections_never_overlap() {
        use ddk_manager::Wallet;

        let wallet = create_wallet().await;
        let addr_one = wallet.new_external_address().await.unwrap().address;
        let addr_two = wallet.new_external_address().await.unwrap().address;
        fund_address(&addr_one);
        fund_address(&addr_two);
        wallet.sync().await.unwrap();

        let amount = Amount::from_btc(0.5).unwrap();
        let first = wallet.get_utxos_for_amount(amount, 1, true).await.unwrap();
        let second = wallet.get_utxos_for_amount(amount, 1, true).await.unwrap();

        assert!(!first.is_empty());
        assert!(!second.is_empty());
        assert!(first.iter().all(|utxo| utxo.reserved));
        for utxo in &first {
            assert!(second.iter().all(|other| other.outpoint != utxo.outpoint));
        }

        // Every coin is locked now: a third selection has nothing to spend.
        assert!(wallet.get_utxos_for_amount(amount, 1, true).await.is_err());
    }

    #[tokio::test]
    async fn unlocked_outpoints_are_selectable_again() {
        use ddk_manager::Wallet;

        let wallet = create_wallet().await;
        let address = wallet.new_external_address().await.unwrap().address;
        fund_address(&address);
        wallet.sync().await.unwrap();

        let amount = Amount::from_btc(0.5).unwrap();
        let selected = wallet.get_utxos_for_amount(amount, 1, true).await.unwrap();
        assert!(wallet.get_utxos_for_amount(amount, 1, true).await.is_err());

        let outpoints = selected
            .iter()
            .map(|utxo| utxo.outpoint)
            .collect::<Vec<_>>();
        wallet.unlock_outpoints(outpoints.clone()).await.unwrap();

        let reselected = wallet.get_utxos_for_amount(amount, 1, true).await.unwrap();
        assert_eq!(
            reselected
                .iter()
                .map(|utxo| utxo.outpoint)
                .collect::<Vec<_>>(),
            outpoints
        );

        // The synchronous manager path unlocks as well. It fires the
        // command without waiting, so the effect lands with the next
        // queued command.
        wallet.unreserve_utxos(&outpoints).unwrap();
        assert!(wallet.get_utxos_for_amount(amount, 1, true).await.is_ok());
    }

    #[tokio::test]
    async fn bump_fee_replaces_a_stuck_transaction() {
        use bitcoincore_rpc::RpcApi;

        // A private environment: a block mined by a concurrent test would
        // confirm the transaction before it can be replaced.
        let env = ddk_testenv::TestEnv::new();
        let mut seed = [0u8; 64];
        seed.try_fill(&mut bitcoin::key::rand::thread_rng())
            .unwrap();
        let wallet = create_wallet_on(env.esplora_host(), &seed).await;
        let address = wallet.new_external_address().await.unwrap().address;
        env.fund_address(&address, Amount::from_btc(1.0).unwrap());
        wallet.sync().await.unwrap();

        let dest = Address::from_str("bcrt1qt0yrvs7qx8guvpqsx8u9mypz6t4zr3pxthsjkm")
            .unwrap()
            .assume_checked();
        let txid = wallet
            .send_to_address(
                dest,
                Amount::from_btc(0.5).unwrap(),
                FeeRate::from_sat_per_vb(1).unwrap(),
            )
            .await
            .unwrap();

        // The wallet has to see its unconfirmed transaction to replace it.
        env.wait_for_tx(&txid);
        wallet.sync().await.unwrap();

        let replacement = wallet
            .bump_fee(txid, FeeRate::from_sat_per_vb(10).unwrap())
            .await
            .unwrap();
        assert_ne!(replacement, txid);

        // The replacement is in the mempool and the original is gone.
        let rpc = env.rpc();
        assert!(rpc.get_mempool_entry(&replacement).is_ok());
        assert!(rpc.get_mempool_entry(&txid).is_err());
    }

    #[tokio::test]
    async fn coin_control_restricts_the_funding_utxos() {
        use bitcoincore_rpc::RpcApi;

        let wallet = create_wallet().await;
        let addr_one = wallet.new_external_address().await.unwrap().address;
        let addr_two = wallet.new_external_address().await.unwrap().address;
        fund_address(&addr_one);
        fund_address(&addr_two);
        wallet.sync().await.unwrap();

        let utxos = wallet.list_utxos().await.unwrap();
        assert_eq!(utxos.len(), 2);
        let selected = utxos[0].outpoint;
        let excluded = utxos[1].outpoint;

        // A manually selected UTXO is the only input of the send.
        let dest = Address::from_str("bcrt1qt0yrvs7qx8guvpqsx8u9mypz6t4zr3pxthsjkm")
            .unwrap()
            .assume_checked();
        let txid = wallet
            .send_with_coin_control(
                dest.clone(),
                super::Spend::Amount(Amount::from_btc(0.5).unwrap()),
                FeeRate::from_sat_per_vb(1).unwrap(),
                super::CoinControl {
                    selected_utxos: vec![selected],
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        let env = ddk_testenv::env();
        let tx = env.rpc().get_raw_transaction(&txid, None).unwrap();
        assert_eq!(tx.input.len(), 1);
        assert_eq!(tx.input[0].previous_output, selected);

        // Let the wallet observe the spend so one coin remains.
        env.wait_for_tx(&txid);
        wallet.sync().await.unwrap();

        // An unspendable outpoint never funds the send: only the other
        // coin remains, so excluding it leaves nothing to spend.
        let result = wallet
            .send_with_coin_control(
                dest,
                super::Spend::Amount(Amount::from_btc(0.5).unwrap()),
                FeeRate::from_sat_per_vb(1).unwrap(),
                super::CoinControl {
                    unspendable: vec![excluded],
                    ..Default::default()
                },
            )
            .await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn emits_wallet_events_on_sync() {
        let wallet = create_wallet().await;
        let mut events = wallet.subscribe_events();
        let address = wallet.new_external_address().await.unwrap().address;
        wallet.sync().await.unwrap();
        while events.try_recv().is_ok() {}

        fund_address(&address);
        wallet.sync().await.unwrap();

        let mut confirmed = false;
        while let Ok(event) = events.try_recv() {
            if matches!(event, super::WalletEvent::TxConfirmed { .. }) {
                confirmed = true;
            }
        }
        assert!(confirmed);
    }

    #[tokio::test]
    async fn balance_splits_spendable_and_reserved() {
        use ddk_manager::Wallet;

        let wallet = create_wallet().await;
        let address = wallet.new_external_address().await.unwrap().address;
        fund_address(&address);
        wallet.sync().await.unwrap();

        let before = wallet.get_balance().await.unwrap();
        assert_eq!(before.reserved, Amount::ZERO);
        assert_eq!(before.spendable, before.confirmed);

        wallet
            .get_utxos_for_amount(Amount::from_btc(0.5).unwrap(), 1, true)
            .await
            .unwrap();

        // The single wallet coin is locked: it moves from spendable to
        // reserved while the total stays the same.
        let after = wallet.get_balance().await.unwrap();
        assert_eq!(after.reserved, before.confirmed);
        assert_eq!(after.spendable, Amount::ZERO);
    }

    #[tokio::test]
    async fn signed_funding_inputs_lock_until_confirmation() {
        use bitcoincore_rpc::RpcApi;
        use ddk_manager::Wallet;

        let wallet = create_wallet().await;
        let address = wallet.new_external_address().await.unwrap().address;
        fund_address(&address);
        wallet.sync().await.unwrap();

        let utxos = wallet.list_utxos().await.unwrap();
        assert_eq!(utxos.len(), 1);
        let utxo = utxos[0].clone();

        // A funding transaction that spends the wallet coin, signed
        // through the manager path but not broadcast.
        let dest = Address::from_str("bcrt1qt0yrvs7qx8guvpqsx8u9mypz6t4zr3pxthsjkm")
            .unwrap()
            .assume_checked();
        let unsigned = bitcoin::Transaction {
            version: bitcoin::transaction::Version::TWO,
            lock_time: bitcoin::absolute::LockTime::ZERO,
            input: vec![bitcoin::TxIn {
                previous_output: utxo.outpoint,
                script_sig: bitcoin::ScriptBuf::new(),
                sequence: bitcoin::Sequence::MAX,
                witness: bitcoin::Witness::new(),
            }],
            output: vec![bitcoin::TxOut {
                value: utxo.txout.value - Amount::from_sat(1_000),
                script_pubkey: dest.script_pubkey(),
            }],
        };
        let mut psbt = bitcoin::Psbt::from_unsigned_tx(unsigned).unwrap();
        psbt.inputs[0].witness_utxo = Some(utxo.txout.clone());

        wallet.sign_psbt_input(&mut psbt, 0).await.unwrap();
        assert!(psbt.inputs[0].final_script_witness.is_some());

        // The signed-but-unbroadcast input is locked and not selectable.
        assert_eq!(
            wallet.locked_outpoints().await.unwrap(),
            vec![utxo.outpoint]
        );
        assert!(wallet
            .get_utxos_for_amount(Amount::from_btc(0.5).unwrap(), 1, true)
            .await
            .is_err());

        // Broadcast and confirm the spend: the sync releases the lock.
        let tx = psbt.extract_tx().unwrap();
        let env = ddk_testenv::env();
        let txid = env.rpc().send_raw_transaction(&tx).unwrap();
        env.wait_for_tx(&txid);
        generate_blocks(1);
        wallet.sync().await.unwrap();
        assert!(wallet.locked_outpoints().await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn utxo_locks_survive_restart() {
        use ddk_manager::Wallet;

        let esplora = ddk_testenv::env().esplora_host().to_string();
        let storage = Arc::new(MemoryStorage::new());
        let mut seed = [0u8; 64];
        seed.try_fill(&mut bitcoin::key::rand::thread_rng())
            .unwrap();

        let wallet = create_wallet_with_storage(&esplora, &seed, storage.clone()).await;
        let address = wallet.new_external_address().await.unwrap().address;
        fund_address(&address);
        wallet.sync().await.unwrap();

        let amount = Amount::from_btc(0.5).unwrap();
        let selected = wallet.get_utxos_for_amount(amount, 1, true).await.unwrap();
        assert!(!selected.is_empty());

        // A wallet loaded from the same storage sees the persisted locks
        // and cannot select the locked coin.
        let restarted = create_wallet_with_storage(&esplora, &seed, storage.clone()).await;
        restarted.sync().await.unwrap();
        assert!(restarted
            .get_utxos_for_amount(amount, 1, true)
            .await
            .is_err());
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

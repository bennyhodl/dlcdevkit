//! # DLC Development Kit (DDK) Core Implementation
//!
//! This module implements the main API and runtime management for DDK, using an actor-based
//! architecture for thread-safe, lock-free DLC operations. The design follows these principles:
//!
//! ## Actor Model Architecture
//! The system uses message passing between components to ensure thread safety and avoid locks:
//! - Components communicate via tokio channels
//! - One-shot channels for request/response patterns
//! - Watch channels for broadcasting state changes
//! - MPSC channels for continuous message streams
//!
//! ## Runtime Management
//! A single tokio runtime is managed to handle:
//! - Transport layer listeners
//! - Wallet synchronization
//! - Contract state monitoring
//! - Background tasks
//!
//! ## Component Integration
//! DDK integrates several components:
//! - Transport layer (Lightning, Nostr, etc.)
//! - Storage backends (PostgreSQL, Sled)
//! - Oracle services
//! - Bitcoin wallet operations
//! - DLC contract management

use crate::chain::EsploraClient;
use crate::error::Error;
use crate::wallet::DlcDevKitWallet;
use crate::{Oracle, Storage, Transport};
use bitcoin::secp256k1::PublicKey;
use bitcoin::{Amount, Network, SignedAmount};
use ddk_manager::contract::Contract;
use ddk_manager::error::Error as ManagerError;
use ddk_manager::{
    contract::contract_input::ContractInput, CachedContractSignerProvider, ContractId,
    SimpleSigner, SystemTimeProvider,
};
use ddk_messages::oracle_msgs::OracleAnnouncement;
use ddk_messages::{AcceptDlc, Message, OfferDlc};
use std::sync::{Arc, RwLock};
use std::time::Duration;
use tokio::runtime::Runtime;
use tokio::sync::mpsc::Sender;
use tokio::sync::oneshot;
use tokio::sync::watch;

/// Type alias for the DLC manager implementation with all its generic parameters.
/// This manager handles the core DLC operations with:
/// - Wallet integration
/// - Contract signing
/// - Blockchain monitoring
/// - Storage operations
/// - Oracle communication
/// - Time management
pub type DlcDevKitDlcManager<S, O> = ddk_manager::manager::Manager<
    Arc<DlcDevKitWallet>,
    Arc<CachedContractSignerProvider<Arc<DlcDevKitWallet>, SimpleSigner>>,
    Arc<EsploraClient>,
    Arc<S>,
    Arc<O>,
    Arc<SystemTimeProvider>,
    Arc<DlcDevKitWallet>,
    SimpleSigner,
>;

type Result<T> = std::result::Result<T, Error>;
type StdResult<T, E> = std::result::Result<T, E>;

/// Messages that can be sent to the DLC manager actor.
/// These messages represent the core operations in the DLC lifecycle:
/// - Offering new contracts
/// - Accepting existing offers
/// - Periodic state checks
///
/// Each operation (except PeriodicCheck) includes a one-shot channel
/// for receiving the operation's result.
#[derive(Debug)]
pub enum DlcManagerMessage {
    /// Accept an existing DLC offer
    AcceptDlc {
        /// Contract ID to accept
        contract: ContractId,
        /// Channel for receiving the acceptance result
        responder: oneshot::Sender<StdResult<(ContractId, PublicKey, AcceptDlc), ManagerError>>,
    },
    /// Create and send a new DLC offer
    OfferDlc {
        /// Contract parameters
        contract_input: ContractInput,
        /// Recipient's public key
        counter_party: PublicKey,
        /// Oracle announcements for the contract
        oracle_announcements: Vec<OracleAnnouncement>,
        /// Channel for receiving the offer result
        responder: oneshot::Sender<StdResult<OfferDlc, ManagerError>>,
    },
    /// Trigger periodic contract state checks
    PeriodicCheck,
}

/// Main DDK instance that encapsulates all DLC functionality.
///
/// This struct manages:
/// 1. Runtime Context:
///    - Single tokio runtime for all async operations
///    - Background task management
///    - Graceful shutdown handling
///
/// 2. Core Components:
///    - Wallet for Bitcoin operations
///    - DLC manager for contract operations
///    - Transport layer for peer communication
///    - Storage backend for persistence
///    - Oracle client for external data
///
/// 3. Communication:
///    - Message channel to the DLC manager actor
///    - Stop signal broadcasting for shutdown coordination
///
/// The struct is designed to be thread-safe and can be shared across
/// multiple threads using Arc.
#[derive(Debug)]
pub struct DlcDevKit<T: Transport, S: Storage, O: Oracle> {
    /// Tokio runtime for async operations
    pub runtime: Arc<RwLock<Option<Runtime>>>,
    /// Bitcoin wallet instance
    pub wallet: Arc<DlcDevKitWallet>,
    /// DLC manager instance
    pub manager: Arc<DlcDevKitDlcManager<S, O>>,
    /// Channel for sending messages to the DLC manager
    pub sender: Sender<DlcManagerMessage>,
    /// Transport layer implementation
    pub transport: Arc<T>,
    /// Storage backend implementation
    pub storage: Arc<S>,
    /// Oracle client implementation
    pub oracle: Arc<O>,
    /// Bitcoin network (mainnet, testnet, regtest)
    pub network: Network,
    /// Receiver for stop signal
    pub stop_signal: watch::Receiver<bool>,
    /// Sender for stop signal
    pub stop_signal_sender: watch::Sender<bool>,
}

impl<T, S, O> DlcDevKit<T, S, O>
where
    T: Transport,
    S: Storage,
    O: Oracle,
{
    /// Starts the DDK runtime with a new multi-threaded tokio runtime.
    /// This spawns all necessary background tasks:
    /// - Transport layer listeners
    /// - Wallet synchronization
    /// - Periodic contract checks
    pub fn start(&self) -> Result<()> {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .unwrap();
        self.start_with_runtime(runtime)
    }

    /// Starts the DDK runtime with a provided tokio runtime.
    /// Useful when integrating with existing async applications.
    ///
    /// This method spawns three critical background tasks:
    ///
    /// 1. Transport Listener Thread:
    /// - Handles incoming DLC messages
    /// - Manages peer connections
    /// - Routes messages to DLC manager
    /// - Gracefully shuts down on stop signal
    ///
    /// 2. Wallet Sync Thread:
    /// - Runs every 60 seconds
    /// - Updates UTXO set
    /// - Syncs with blockchain
    /// - Maintains wallet state
    ///
    /// 3. Contract Monitor Thread:
    /// - Runs every 30 seconds
    /// - Checks contract states
    /// - Triggers necessary updates
    /// - Maintains contract lifecycle
    ///
    /// # Arguments
    /// * `runtime` - A tokio runtime to use for async operations
    ///
    /// # Returns
    /// * `Ok(())` if runtime started successfully
    /// * `Err(Error::RuntimeExists)` if runtime is already running
    pub fn start_with_runtime(&self, runtime: Runtime) -> Result<()> {
        let mut runtime_lock = self.runtime.write().unwrap();

        if runtime_lock.is_some() {
            return Err(Error::RuntimeExists);
        }

        // Spawn transport listener thread
        let transport_clone = self.transport.clone();
        let manager_clone = self.manager.clone();
        let stop_signal = self.stop_signal.clone();
        runtime.spawn(async move {
            if let Err(e) = transport_clone.start(stop_signal, manager_clone).await {
                tracing::error!(error = e.to_string(), "Error in transport listeners.");
            }
        });

        // Spawn wallet sync thread (60-second interval)
        let wallet_clone = self.wallet.clone();
        runtime.spawn(async move {
            let mut timer = tokio::time::interval(Duration::from_secs(60));
            loop {
                timer.tick().await;
                if let Err(e) = wallet_clone.sync().await {
                    tracing::warn!(error=?e, "Did not sync wallet.");
                };
            }
        });

        // Spawn contract monitor thread (30-second interval)
        let processor = self.sender.clone();
        runtime.spawn(async move {
            let mut timer = tokio::time::interval(Duration::from_secs(30));
            loop {
                timer.tick().await;
                let _ = processor
                    .send(DlcManagerMessage::PeriodicCheck)
                    .await
                    .map_err(|e| {
                        tracing::error!("Error sending periodic check: {}", e);
                    });
            }
        });

        *runtime_lock = Some(runtime);
        Ok(())
    }

    /// Gracefully stops the DDK runtime and all background tasks.
    /// This ensures:
    /// - All listeners are closed
    /// - Background tasks are terminated
    /// - Resources are properly cleaned up
    pub fn stop(&self) -> Result<()> {
        tracing::warn!("Shutting down DDK runtime and listeners.");
        self.stop_signal_sender
            .send(true)
            .map_err(|e| Error::ActorSendError(e.to_string()))?;
        let mut runtime_lock = self.runtime.write().unwrap();
        if let Some(rt) = runtime_lock.take() {
            rt.shutdown_background();
            Ok(())
        } else {
            Err(Error::NoRuntime)
        }
    }

    /// Returns the configured Bitcoin network
    pub fn network(&self) -> Network {
        self.network
    }

    /// Creates and sends a new DLC offer to a counterparty.
    ///
    /// This method:
    /// 1. Creates a DLC offer message
    /// 2. Sends it through the transport layer
    /// 3. Returns the created offer for further processing
    pub async fn send_dlc_offer(
        &self,
        contract_input: &ContractInput,
        counter_party: PublicKey,
        oracle_announcements: Vec<OracleAnnouncement>,
    ) -> Result<OfferDlc> {
        let (responder, receiver) = oneshot::channel();
        self.sender
            .send(DlcManagerMessage::OfferDlc {
                contract_input: contract_input.to_owned(),
                counter_party,
                oracle_announcements,
                responder,
            })
            .await
            .map_err(|e| Error::ActorSendError(e.to_string()))?;
        let offer = receiver
            .await
            .map_err(|e| Error::ActorReceiveError(e.to_string()))?;

        let offer = offer?;

        let contract_id = hex::encode(offer.temporary_contract_id);
        self.transport
            .send_message(counter_party, Message::Offer(offer.clone()))
            .await;
        tracing::info!(
            counterparty = counter_party.to_string(),
            contract_id,
            "Sent DLC offer to counterparty."
        );

        Ok(offer)
    }

    /// Accepts an existing DLC offer.
    ///
    /// This method:
    /// 1. Processes the acceptance
    /// 2. Creates acceptance message
    /// 3. Sends it to the counterparty
    /// 4. Returns the acceptance details
    pub async fn accept_dlc_offer(
        &self,
        contract: [u8; 32],
    ) -> Result<(String, String, AcceptDlc)> {
        let (responder, receiver) = oneshot::channel();
        self.sender
            .send(DlcManagerMessage::AcceptDlc {
                contract,
                responder,
            })
            .await
            .map_err(|e| Error::ActorSendError(e.to_string()))?;

        let received_message = receiver
            .await
            .map_err(|e| Error::ActorReceiveError(e.to_string()))?;

        let (contract_id, public_key, accept_dlc) = received_message?;

        self.transport
            .send_message(public_key, Message::Accept(accept_dlc.clone()))
            .await;

        let contract_id = hex::encode(contract_id);
        let counter_party = public_key.to_string();
        tracing::info!(
            counter_party,
            contract_id,
            "Accepted and sent accept DLC contract."
        );

        Ok((contract_id, counter_party, accept_dlc))
    }

    /// Retrieves the current balance state, including:
    /// - Confirmed balance
    /// - Unconfirmed changes
    /// - Funds locked in contracts
    /// - Total profit/loss from closed contracts
    pub async fn balance(&self) -> Result<crate::Balance> {
        let wallet_balance = self.wallet.get_balance().await?;
        let contracts = self.storage.get_contracts().await?;

        let contract = &contracts
            .iter()
            .map(|contract| match contract {
                Contract::Confirmed(c) => {
                    let accept_party_collateral = c.accepted_contract.accept_params.collateral;
                    let total_collateral = c.accepted_contract.offered_contract.total_collateral;
                    if c.accepted_contract.offered_contract.is_offer_party {
                        total_collateral - accept_party_collateral
                    } else {
                        accept_party_collateral
                    }
                }
                _ => Amount::ZERO,
            })
            .sum::<Amount>();

        let contract_pnl = &contracts
            .iter()
            .map(|contract| contract.get_pnl())
            .sum::<SignedAmount>();

        Ok(crate::Balance {
            confirmed: wallet_balance.confirmed,
            change_unconfirmed: wallet_balance.immature + wallet_balance.trusted_pending,
            foreign_unconfirmed: wallet_balance.untrusted_pending,
            contract: contract.to_owned(),
            contract_pnl: contract_pnl.to_owned().to_sat(),
        })
    }
}

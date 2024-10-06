use crate::io;
use bitcoin::Network;
use core::fmt;
use crossbeam::channel::unbounded;
use dlc_manager::manager::Manager;
use dlc_manager::SystemTimeProvider;
use std::collections::HashMap;
use std::sync::{Arc, RwLock};

use crate::chain::EsploraClient;
use crate::config::SeedConfig;
use crate::ddk::{DlcDevKit, DlcManagerMessage};
use crate::wallet::DlcDevKitWallet;
use crate::{DdkOracle, DdkStorage, DdkTransport};

pub const DEFAULT_STORAGE_PATH: &str = "/tmp/ddk";
pub const DEFAULT_ESPLORA_HOST: &str = "https://mutinynet.com/api";
pub const DEFAULT_NETWORK: Network = Network::Signet;

/// Builder pattern for creating a [crate::ddk::DlcDevKit] process.
#[derive(Clone, Debug)]
pub struct DdkBuilder<T, S, O> {
    name: Option<String>,
    transport: Option<Arc<T>>,
    storage: Option<Arc<S>>,
    oracle: Option<Arc<O>>,
    wallet_storage: Option<S>,
    esplora_host: String,
    network: Network,
    storage_path: String,
}

/// An error that could be thrown while building [crate::ddk::DlcDevKit]
#[derive(Debug, Clone, Copy)]
pub enum BuilderError {
    /// A transport was not provided.
    NoTransport,
    /// A storage implementation was not provided.
    NoStorage,
    /// An oracle client was not provided.
    NoOracle,
    /// No seed provided
    NoSeed,
    /// No wallet storage provided.
    NoWalletStorage,
}

impl fmt::Display for BuilderError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            BuilderError::NoTransport => write!(f, "A DLC transport was not provided."),
            BuilderError::NoStorage => write!(f, "A DLC storage implementation was not provided."),
            BuilderError::NoOracle => write!(f, "A DLC oracle client was not provided."),
            BuilderError::NoSeed => write!(f, "No seed configuration was provided."),
            BuilderError::NoWalletStorage => write!(f, "No wallet storage was provided."),
        }
    }
}

impl std::error::Error for BuilderError {}

/// Defaults when creating a DDK application
/// Transport, storage, and oracle is set to none.
/// Default [crate::config::DdkConfig] to mutiny net.
impl<T: DdkTransport, S: DdkStorage, O: DdkOracle> Default for DdkBuilder<T, S, O> {
    fn default() -> Self {
        Self {
            name: None,
            transport: None,
            storage: None,
            oracle: None,
            wallet_storage: None,
            esplora_host: DEFAULT_ESPLORA_HOST.to_string(),
            network: DEFAULT_NETWORK,
            storage_path: DEFAULT_STORAGE_PATH.to_string(),
        }
    }
}

impl<T: DdkTransport, S: DdkStorage, O: DdkOracle> DdkBuilder<T, S, O> {
    /// Create a new, default DDK builder.
    pub fn new() -> Self {
        DdkBuilder::default()
    }

    /// Set the name of the DDK process. Used as an identifier for the process created.
    /// Creates a directory for the process with the name specifed. All file-based components
    /// will be stored in a directory under the storage path set in the `DdkConfig` and the `name`.
    /// If no name is set, defaults to a generated `uuid`.
    pub fn set_name(&mut self, name: &str) -> &mut Self {
        self.name = Some(name.into());
        self
    }

    /// The communication layer of DDK. Type MUST implement [crate::DdkTransport].
    /// Transport sets up listeners, communicates with counterparties, and passes
    /// DLC messages to the `Manager`.
    pub fn set_transport(&mut self, transport: Arc<T>) -> &mut Self {
        self.transport = Some(transport);
        self
    }

    /// DLC contract storage. Storage is used by the [dlc_manager::manager::Manager] to create, update, retrieve, and
    /// delete contracts. MUST implement [crate::DdkStorage]
    pub fn set_storage(&mut self, storage: Arc<S>) -> &mut Self {
        self.storage = Some(storage);
        self
    }

    /// DLC wallet storage. Storage is used by the [bdk::wallet::Wallet] to create, update, retrieve, and
    /// delete wallet information. MUST implement [bdk_chain::PersistBackend<bdk::wallet::ChangeSet>]
    ///
    /// TODO: Figure out a way to pass storage wrapped in arc to `Wallet::new_or_load()`..
    pub fn set_wallet_storage(&mut self, wallet_storage: S) -> &mut Self {
        self.wallet_storage = Some(wallet_storage);
        self
    }

    /// Oracle implementation for the [dlc_manager::manager::Manager] to retrieve oracle attestations and announcements.
    /// MUST implement [crate::DdkOracle].
    pub fn set_oracle(&mut self, oracle: Arc<O>) -> &mut Self {
        self.oracle = Some(oracle);
        self
    }

    /// Set the esplora server to connect to.
    pub fn set_esplora_host(&mut self, host: String) -> &mut Self {
        self.esplora_host = host;
        self
    }

    /// Set the network DDK connects to.
    pub fn set_network(&mut self, network: Network) -> &mut Self {
        self.network = network;
        self
    }

    /// Storage path to store DDK related data.
    pub fn set_storage_path(&mut self, path: String) -> &mut Self {
        self.storage_path = path;
        self
    }

    /// Builds the `DlcDevKit` instance. Fails if any components are missing.
    pub fn finish(&self) -> anyhow::Result<DlcDevKit<T, S, O>> {
        tracing::info!("Using network {}", &self.network);

        // Creates the DDK directory.
        //
        // TODO: Should have a storage config for no-std builds.
        // TODO: should be nested with the DDK name.
        std::fs::create_dir_all(&self.storage_path)?;
        tracing::info!(path=?self.storage_path, "Created directory for ddk node.");

        let xprv =
            io::xprv_from_config(&SeedConfig::File(self.storage_path.clone()), self.network)?;
        tracing::info!("Loaded private key");

        let transport = self
            .transport
            .as_ref()
            .map_or_else(|| Err(BuilderError::NoTransport), |t| Ok(t.clone()))?;

        let storage = self
            .storage
            .as_ref()
            .map_or_else(|| Err(BuilderError::NoStorage), |s| Ok(s.clone()))?;

        let oracle = self
            .oracle
            .as_ref()
            .map_or_else(|| Err(BuilderError::NoOracle), |o| Ok(o.clone()))?;

        let name = self
            .name
            .clone()
            .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());

        let wallet = Arc::new(DlcDevKitWallet::new(
            &name,
            xprv,
            &self.esplora_host,
            self.network,
            &self.storage_path,
            storage.clone(),
        )?);
        tracing::info!("Opened BDK wallet. name={}", name);

        let mut oracles = HashMap::new();
        oracles.insert(oracle.get_public_key(), oracle.clone());
        tracing::info!(name = oracle.name(), "Connected to oracle.");

        let esplora_client = Arc::new(EsploraClient::new(&self.esplora_host, self.network)?);
        tracing::info!(host = self.esplora_host, "Connected to esplora client.");

        let (sender, receiver) = unbounded::<DlcManagerMessage>();

        let manager = Arc::new(Manager::new(
            wallet.clone(),
            wallet.clone(),
            esplora_client.clone(),
            storage.clone(),
            oracles,
            Arc::new(SystemTimeProvider {}),
            wallet.clone(),
        )?);
        tracing::info!("Created ddk dlc manager.");

        Ok(DlcDevKit {
            runtime: Arc::new(RwLock::new(None)),
            wallet,
            manager,
            sender: Arc::new(sender),
            receiver: Arc::new(receiver),
            transport,
            storage,
            oracle,
            network: self.network,
        })
    }
}

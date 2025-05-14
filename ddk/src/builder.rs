use bitcoin::Network;
use crossbeam::channel::unbounded;
use ddk_manager::manager::Manager;
use ddk_manager::SystemTimeProvider;
use std::collections::HashMap;
use std::sync::{Arc, RwLock};

use crate::chain::EsploraClient;
use crate::ddk::{DlcDevKit, DlcManagerMessage};
use crate::error::{BuilderError, Error};
use crate::wallet::DlcDevKitWallet;
use crate::{Oracle, Storage, Transport};

const DEFAULT_ESPLORA_HOST: &str = "https://mutinynet.com/api";
const DEFAULT_NETWORK: Network = Network::Signet;

/// Builder pattern for creating a [`crate::ddk::DlcDevKit`] process.
#[derive(Clone, Debug)]
pub struct Builder<T, S, O> {
    name: Option<String>,
    transport: Option<Arc<T>>,
    storage: Option<Arc<S>>,
    oracle: Option<Arc<O>>,
    esplora_host: String,
    network: Network,
    seed_bytes: [u8; 32],
}

/// Defaults when creating a DDK application
/// Transport, storage, and oracle is set to none.
///
/// esplora_host: <https://mutinynet.com/api>
/// network: Network::Signet
impl<T: Transport, S: Storage, O: Oracle> Default for Builder<T, S, O> {
    fn default() -> Self {
        Self {
            name: None,
            transport: None,
            storage: None,
            oracle: None,
            esplora_host: DEFAULT_ESPLORA_HOST.to_string(),
            network: DEFAULT_NETWORK,
            seed_bytes: [0u8; 32],
        }
    }
}

impl<T: Transport, S: Storage, O: Oracle> Builder<T, S, O> {
    /// Create a new, default DDK builder.
    pub fn new() -> Self {
        Builder::default()
    }

    /// Set the name of the DDK process. Used as an identifier for the process created.
    /// Creates a directory for the process with the name specifed. All file-based components
    /// will be stored in a directory under the storage path set in the `DdkConfig` and the `name`.
    /// If no name is set, defaults to a generated `uuid`.
    pub fn set_name(&mut self, name: &str) -> &mut Self {
        self.name = Some(name.into());
        self
    }

    /// The communication layer of DDK. Type MUST implement [crate::Transport].
    /// Transport sets up listeners, communicates with counterparties, and passes
    /// DLC messages to the `Manager`.
    pub fn set_transport(&mut self, transport: Arc<T>) -> &mut Self {
        self.transport = Some(transport);
        self
    }

    /// DLC contract storage. Storage is used by the [ddk_manager::manager::Manager] to create, update, retrieve, and
    /// delete contracts. MUST implement [crate::Storage]
    pub fn set_storage(&mut self, storage: Arc<S>) -> &mut Self {
        self.storage = Some(storage);
        self
    }

    /// Oracle implementation for the [ddk_manager::manager::Manager] to retrieve oracle attestations and announcements.
    /// MUST implement [crate::Oracle].
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

    /// Set the seed bytes for the wallet.
    pub fn set_seed_bytes(&mut self, bytes: [u8; 32]) -> &mut Self {
        self.seed_bytes = bytes;
        self
    }

    /// Builds the `DlcDevKit` instance. Fails if any components are missing.
    pub async fn finish(&self) -> Result<DlcDevKit<T, S, O>, Error> {
        tracing::info!(
            network = self.network.to_string(),
            esplora = self.esplora_host,
            "Building DDK."
        );

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

        let wallet = Arc::new(
            DlcDevKitWallet::new(
                &name,
                &self.seed_bytes,
                &self.esplora_host,
                self.network,
                storage.clone(),
            )
            .await?,
        );

        let mut oracles = HashMap::new();
        oracles.insert(oracle.get_public_key(), oracle.clone());

        let esplora_client = Arc::new(EsploraClient::new(&self.esplora_host, self.network)?);

        let (sender, receiver) = unbounded::<DlcManagerMessage>();
        let (stop_signal_sender, stop_signal) = tokio::sync::watch::channel(false);

        let manager = Arc::new(
            Manager::new(
                wallet.clone(),
                wallet.clone(),
                esplora_client.clone(),
                storage.clone(),
                oracles,
                Arc::new(SystemTimeProvider {}),
                wallet.clone(),
            )
            .await?,
        );
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
            stop_signal,
            stop_signal_sender,
        })
    }
}

use crate::{io, SeedConfig};
use bdk::chain::PersistBackend;
use bdk::wallet::ChangeSet;
use core::fmt;
use dlc_manager::manager::Manager;
use dlc_manager::SystemTimeProvider;
use std::collections::HashMap;
use std::sync::{Arc, Mutex, RwLock};

use crate::chain::EsploraClient;
use crate::config::DdkConfig;
use crate::ddk::DlcDevKit;
use crate::wallet::DlcDevKitWallet;
use crate::{DdkOracle, DdkStorage, DdkTransport};

#[derive(Clone, Debug)]
pub struct DdkBuilder<T, S, O> {
    name: Option<String>,
    config: Option<DdkConfig>,
    transport: Option<Arc<T>>,
    storage: Option<Arc<S>>,
    oracle: Option<Arc<O>>,
}

/// An error that could be thrown while building [`DlcDevKit`]
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
    /// No config provided.
    NoConfig,
}

impl fmt::Display for BuilderError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            BuilderError::NoTransport => write!(f, "A DLC transport was not provided."),
            BuilderError::NoStorage => write!(f, "A DLC storage implementation was not provided."),
            BuilderError::NoOracle => write!(f, "A DLC oracle client was not provided."),
            BuilderError::NoSeed => write!(f, "No seed configuration was provided."),
            BuilderError::NoConfig => write!(f, "No config was provided"),
        }
    }
}

impl std::error::Error for BuilderError {}

impl<T: DdkTransport, S: DdkStorage, O: DdkOracle> Default for DdkBuilder<T, S, O> {
    fn default() -> Self {
        let config = Some(DdkConfig::default());
        Self {
            name: None,
            config,
            transport: None,
            storage: None,
            oracle: None,
        }
    }
}

impl<T: DdkTransport, S: DdkStorage, O: DdkOracle> DdkBuilder<T, S, O> {
    pub fn new() -> Self {
        DdkBuilder::default()
    }

    pub fn set_name(&mut self, name: &str) -> &mut Self {
        self.name = Some(name.into());
        self
    }

    pub fn set_transport(&mut self, transport: Arc<T>) -> &mut Self {
        self.transport = Some(transport);
        self
    }

    pub fn set_storage(&mut self, storage: Arc<S>) -> &mut Self {
        self.storage = Some(storage);
        self
    }

    pub fn set_oracle(&mut self, oracle: Arc<O>) -> &mut Self {
        self.oracle = Some(oracle);
        self
    }

    pub fn set_config(&mut self, config: DdkConfig) -> &mut Self {
        self.config = Some(config);
        self
    }

    pub fn finish(&self) -> anyhow::Result<DlcDevKit<T, S, O>> {
        let config = self
            .config
            .as_ref()
            .map_or_else(|| Err(BuilderError::NoConfig), |c| Ok(c))?;

        std::fs::create_dir_all(&config.storage_path)?;

        let xprv = io::xprv_from_config(&config.seed_config, config.network)?;

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

        tracing::info!("Creating new P2P DlcDevKit wallet. name={}", name);
        let wallet = Arc::new(DlcDevKitWallet::new(
            &name,
            xprv,
            &config.esplora_host,
            config.network,
            &config.storage_path,
        )?);

        let mut oracles = HashMap::new();
        oracles.insert(oracle.get_public_key(), oracle.clone());

        let esplora_client = Arc::new(EsploraClient::new(&config.esplora_host, config.network)?);

        let manager = Arc::new(Mutex::new(Manager::new(
            wallet.clone(),
            wallet.clone(),
            esplora_client.clone(),
            storage.clone(),
            oracles,
            Arc::new(SystemTimeProvider {}),
            wallet.clone(),
        )?));

        Ok(DlcDevKit {
            runtime: Arc::new(RwLock::new(None)),
            wallet,
            manager,
            transport,
            storage,
            oracle,
            network: config.network,
        })
    }
}

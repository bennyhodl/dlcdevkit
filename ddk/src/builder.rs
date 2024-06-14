use crate::{io, SeedConfig};
use core::fmt;
use bdk::chain::PersistBackend;
use bdk::wallet::ChangeSet;
use dlc_manager::manager::Manager;
use dlc_manager::SystemTimeProvider;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::Mutex;

use crate::chain::EsploraClient;
use crate::config::DdkConfig;
use crate::ddk::DlcDevKit;
use crate::wallet::DlcDevKitWallet;
use crate::{DdkOracle, DdkStorage, DdkTransport};

#[derive(Clone, Debug)]
pub struct DdkBuilder<T, S, O, WS> {
    name: Option<String>,
    config: Option<DdkConfig>,
    seed: Option<SeedConfig>,
    transport: Option<Arc<T>>,
    storage: Option<Arc<S>>,
    wallet_storage: Option<WS>,
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

impl<T: DdkTransport, S: DdkStorage, O: DdkOracle, WS: PersistBackend<ChangeSet>> Default for DdkBuilder<T, S, O, WS> {
    fn default() -> Self {
        Self {
            name: None,
            config: None,
            seed: None,
            transport: None,
            storage: None,
            wallet_storage: None,
            oracle: None,
        }
    }
}

impl<T: DdkTransport, S: DdkStorage, O: DdkOracle, WS: PersistBackend<ChangeSet> + Clone + Copy> DdkBuilder<T, S, O, WS> {
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

    pub fn set_seed_config(&mut self, seed_config: SeedConfig) -> &mut Self {
        self.seed = Some(seed_config);
        self
    }

    pub fn set_wallet_storage(&mut self, wallet_storage: WS) -> &mut Self {
        self.wallet_storage = Some(wallet_storage);
        self
    }

    pub async fn finish(&self) -> anyhow::Result<DlcDevKit<T, S, O, WS>> {
        let config = self
            .config
            .as_ref()
            .map_or_else(|| Err(BuilderError::NoConfig), |c| Ok(c))?;
        let seed = self
            .seed
            .as_ref()
            .map_or_else(|| Err(BuilderError::NoSeed), |s| Ok(s))?;
        println!("Creating {:?}", config.storage_path);
        std::fs::create_dir_all(&config.storage_path)?;
        let xprv = io::xprv_from_config(&seed, config.network)?;

        let transport = self
            .transport
            .as_ref()
            .map_or_else(|| Err(BuilderError::NoTransport), |t| Ok(t.clone()))?;

        let storage = self
            .storage
            .as_ref()
            .map_or_else(|| Err(BuilderError::NoStorage), |s| Ok(s.clone()))?;

        let wallet_storage = self
            .wallet_storage
            .unwrap();

        let oracle = self
            .oracle
            .as_ref()
            .map_or_else(|| Err(BuilderError::NoOracle), |o| Ok(o.clone()))?;

        let name = match self.name.clone() {
            Some(n) => n,
            None => uuid::Uuid::new_v4().to_string(),
        };

        log::info!("Creating new P2P DlcDevKit wallet. name={}", name);
        let wallet = Arc::new(DlcDevKitWallet::new(
            &name,
            xprv,
            &config.esplora_host,
            config.network,
            wallet_storage,
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
            wallet,
            manager,
            transport,
            storage,
            oracle,
        })
    }
}

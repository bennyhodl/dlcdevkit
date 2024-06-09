use core::fmt;
use std::collections::HashMap;
use std::sync::Arc;
use bitcoin::bip32::ExtendedPrivKey;
use tokio::sync::Mutex;

use crate::oracle::P2PDOracleClient;
use crate::storage::SledStorageProvider;
use bitcoin::Network;
use dlc_manager::manager::Manager;
use dlc_manager::Oracle;
use dlc_manager::SystemTimeProvider;

use crate::chain::EsploraClient;
use crate::ddk::DlcDevKit;
use crate::transport::lightning::LightningTransport;
use crate::wallet::DlcDevKitWallet;
use crate::{get_dlc_dev_kit_dir, DdkOracle, DdkStorage, DdkTransport, ORACLE_HOST};

#[derive(Debug, Clone)]
pub enum SeedConfig {
    Bytes([u8;64]),
    File(String),
}

#[derive(Clone, Debug)]
pub struct DdkBuilder<T, S, O> {
    name: Option<String>,
    transport: Option<Arc<T>>,
    storage: Option<Arc<S>>,
    oracle: Option<Arc<O>>,
    esplora_url: String,
    network: Network,
    seed: Option<SeedConfig>
    // entropy config
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
}

impl fmt::Display for BuilderError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            BuilderError::NoTransport => write!(f, "A DLC transport was not provided."),
            BuilderError::NoStorage => write!(f, "A DLC storage implementation was not provided."),
            BuilderError::NoOracle => write!(f, "A DLC oracle client was not provided."),
            BuilderError::NoSeed => write!(f, "No seed configuration was provided.")
        }
    }
}

impl std::error::Error for BuilderError {}

impl<T: DdkTransport, S: DdkStorage, O: DdkOracle> Default for DdkBuilder<T, S, O> {
    fn default() -> Self {
        Self {
            name: None,
            transport: None,
            storage: None,
            oracle: None,
            esplora_url: "https://mutinynet.com/api".into(),
            network: Network::Regtest,
            seed: None
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

    pub fn set_esplora_url(&mut self, esplora_url: &str) -> &mut Self {
        self.esplora_url = esplora_url.into();
        self
    }

    pub fn set_network(&mut self, network: Network) -> &mut Self {
        self.network = network;
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

    pub fn set_seed_config(&mut self, seed_config: SeedConfig) -> &mut Self {
        self.seed = Some(seed_config);
        self
    }

    pub async fn finish(&self) -> anyhow::Result<DlcDevKit<T, S, O>> {
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

        let name = match self.name.clone() {
            Some(n) => n,
            None => uuid::Uuid::new_v4().to_string(),
        };

        let seed_config = self.seed.as_ref().map_or_else(|| Err(BuilderError::NoSeed), |s| Ok(s.clone()))?;

        let xprv = xprv_from_config(seed_config, self.network)?;

        log::info!("Creating new P2P DlcDevKit wallet. name={}", name);
        let wallet = Arc::new(DlcDevKitWallet::new(
            &name,
            xprv,
            &self.esplora_url,
            self.network,
        )?);

        let db_path = get_dlc_dev_kit_dir().join(&name);
        let dlc_storage = Box::new(SledStorageProvider::new(db_path.to_str().unwrap())?);

        let oracle_internal = 
            tokio::task::spawn_blocking(move || P2PDOracleClient::new(ORACLE_HOST).unwrap()).await.unwrap();

        let mut oracles = HashMap::new();
        oracles.insert(oracle_internal.get_public_key(), Box::new(oracle_internal));

        let esplora_client = Arc::new(EsploraClient::new(&self.esplora_url, self.network)?);

        let manager = Arc::new(Mutex::new(Manager::new(
            wallet.clone(),
            wallet.clone(),
            esplora_client.clone(),
            dlc_storage,
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

/// TODO: Builder error
fn xprv_from_config(seed_config: SeedConfig, network: Network) -> anyhow::Result<ExtendedPrivKey> {
    let seed = match seed_config {
        SeedConfig::Bytes(bytes) => ExtendedPrivKey::new_master(network, &bytes)?,
        SeedConfig::File(file) => {
            let seed = std::fs::read(file)?;
            let mut key = [0; 64];
            key.copy_from_slice(&seed);

            ExtendedPrivKey::new_master(network, &key)?
        }
    };

    Ok(seed)
}

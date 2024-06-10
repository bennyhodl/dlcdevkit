use core::fmt;
use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use bitcoin::bip32::ExtendedPrivKey;
use tokio::sync::Mutex;
use getrandom::getrandom;
use crate::oracle::P2PDOracleClient;
use crate::storage::SledStorageProvider;
use bitcoin::Network;
use dlc_manager::manager::Manager;
use dlc_manager::Oracle;
use dlc_manager::SystemTimeProvider;

use crate::chain::EsploraClient;
use crate::config::{DdkConfig, SeedConfig};
use crate::ddk::DlcDevKit;
use crate::transport::lightning::LightningTransport;
use crate::wallet::DlcDevKitWallet;
use crate::{get_dlc_dev_kit_dir, DdkOracle, DdkStorage, DdkTransport, ORACLE_HOST};

#[derive(Clone, Debug)]
pub struct DdkBuilder<T, S, O> {
    name: Option<String>,
    config: DdkConfig,
    transport: Option<Arc<T>>,
    storage: Option<Arc<S>>,
    oracle: Option<Arc<O>>,
    esplora_url: String,
    network: Network,
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
            config: DdkConfig::default()
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

    pub fn set_config(&mut self, config: DdkConfig) -> &mut Self {
        self.config = config;
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

        let seed_config = self.config.seed();

        let xprv = xprv_from_config(seed_config, self.network)?;

        log::info!("Creating new P2P DlcDevKit wallet. name={}", name);
        let wallet = Arc::new(DlcDevKitWallet::new(
            &name,
            xprv,
            &self.esplora_url,
            self.network,
        )?);

        let oracle_internal = 
            tokio::task::spawn_blocking(move || P2PDOracleClient::new(ORACLE_HOST).unwrap()).await.unwrap();

        let mut oracles = HashMap::new();
        oracles.insert(oracle_internal.get_public_key(), Box::new(oracle_internal));

        let esplora_client = Arc::new(EsploraClient::new(&self.esplora_url, self.network)?);

        let manager = Arc::new(Mutex::new(Manager::new(
            wallet.clone(),
            wallet.clone(),
            esplora_client.clone(),
            Box::new(storage),
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

fn xprv_from_config(seed_config: SeedConfig, network: Network) -> anyhow::Result<ExtendedPrivKey> {
    let seed = match seed_config {
        SeedConfig::Bytes(bytes) => ExtendedPrivKey::new_master(network, &bytes)?,
        SeedConfig::File(file) => {
            if Path::new(&format!("{file}/seed")).exists() {
                let seed = std::fs::read(format!("{file}/seed"))?;
                let mut key = [0; 64];
                key.copy_from_slice(&seed);
                let xprv = ExtendedPrivKey::new_master(network, &seed)?;
                xprv
            } else {
                std::fs::File::create(format!("{file}/seed"))?;
                let mut entropy = [0u8; 78];
                getrandom(&mut entropy)?;
                // let _mnemonic = Mnemonic::from_entropy(&entropy)?;
                let xprv = ExtendedPrivKey::new_master(network, &entropy)?;
                std::fs::write(format!("{file}/seed"), &xprv.encode())?;
                xprv
            }
        }
    };

    Ok(seed)
}

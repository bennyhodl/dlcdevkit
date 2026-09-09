use crate::logger::{log_error, log_info, WriteLog};
use bip39::{Language, Mnemonic};
use bitcoin::key::rand::Fill;
use bitcoin::Network;
use ddk_manager::manager::{CooperativeCloseApprover, Manager};
use ddk_manager::SystemTimeProvider;
use std::collections::HashMap;
use std::sync::{Arc, RwLock};

use crate::chain::{EsploraClient, ZeromqClient};
use crate::ddk::{group_announcements, DlcDevKit, DlcManagerMessage};
use crate::error::{BuilderError, Error};
use crate::logger::{LogLevel, Logger};
use crate::wallet::address::AddressGenerator;
use crate::wallet::DlcDevKitWallet;
use crate::{Oracle, Storage, Transport};

const DEFAULT_ESPLORA_HOST: &str = "https://mutinynet.com/api";
const DEFAULT_NETWORK: Network = Network::Signet;
const DEFAULT_LOG_LEVEL: LogLevel = LogLevel::Info;

/// Configuration for the seed bytes for the wallet.
#[derive(Debug, Clone)]
pub enum SeedConfig {
    /// Generates a random seed everytime ddk is run.
    Random,
    /// The first string is the mnemonic, the second is the passphrase.
    Mnemonic(String, String),
    /// The bytes to use for the seed.
    Bytes([u8; 64]),
}

/// Builder pattern for creating a [`crate::ddk::DlcDevKit`] process.
#[derive(Clone)]
pub struct Builder<T, S, O> {
    name: Option<String>,
    transport: Option<Arc<T>>,
    storage: Option<Arc<S>>,
    oracle: Option<Arc<O>>,
    contract_address_generator: Option<Arc<dyn AddressGenerator + Send + Sync + 'static>>,
    esplora_host: String,
    zmq_blockhash_endpoint: Option<String>,
    network: Network,
    seed_bytes: Option<[u8; 64]>,
    wallet_config: crate::wallet::WalletConfig,
    logger: Option<Arc<Logger>>,
    close_approver: Option<Arc<dyn CooperativeCloseApprover>>,
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
            contract_address_generator: None,
            esplora_host: DEFAULT_ESPLORA_HOST.to_string(),
            zmq_blockhash_endpoint: None,
            network: DEFAULT_NETWORK,
            seed_bytes: None,
            wallet_config: crate::wallet::WalletConfig::default(),
            logger: None,
            close_approver: None,
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

    /// DLC contract storage. Storage is used by the [`ddk_manager::manager::Manager`] to create, update, retrieve, and
    /// delete contracts. MUST implement [`crate::Storage`].
    pub fn set_storage(&mut self, storage: Arc<S>) -> &mut Self {
        self.storage = Some(storage);
        self
    }

    /// Oracle implementation for the [ddk_manager::manager::Manager] to retrieve oracle attestations and announcements.
    /// MUST implement [`crate::Oracle`].
    pub fn set_oracle(&mut self, oracle: Arc<O>) -> &mut Self {
        self.oracle = Some(oracle);
        self
    }

    /// Wallet implementation for the [ddk_manager::manager::Manager] to retrieve wallet keys and sign transactions.
    /// For now, just uses the [`DlcDevKitWallet`] implementation. Optionally can use a custom address generation.
    pub fn set_contract_address_generator(
        &mut self,
        contract_address_generator: Arc<dyn AddressGenerator + Send + Sync + 'static>,
    ) -> &mut Self {
        self.contract_address_generator = Some(contract_address_generator);
        self
    }

    /// Set the esplora server to connect to.
    pub fn set_esplora_host(&mut self, host: String) -> &mut Self {
        self.esplora_host = host;
        self
    }

    /// Set the bitcoind server to connect to.
    pub fn set_zmq_blockhash_endpoint(&mut self, endpoint: impl ToString) -> &mut Self {
        self.zmq_blockhash_endpoint = Some(endpoint.to_string());
        self
    }

    /// Set the network DDK connects to.
    pub fn set_network(&mut self, network: Network) -> &mut Self {
        self.network = network;
        self
    }

    /// Set the seed bytes for the wallet.
    ///
    /// The seed is the root of the wallet keys and every DLC contract key.
    /// It is required: [`Builder::finish`] fails with [`BuilderError::NoSeed`]
    /// when it was never set, and the wallet rejects an all-zero seed.
    pub fn set_seed_bytes(&mut self, seed_config: SeedConfig) -> Result<&mut Self, BuilderError> {
        let seed = match seed_config {
            SeedConfig::Random => {
                let mut seed = [0u8; 64];
                seed.try_fill(&mut bitcoin::key::rand::thread_rng())
                    .map_err(|_| BuilderError::SeedGenerationFailed)?;
                seed
            }
            SeedConfig::Mnemonic(mnemonic, passphrase) => {
                let mnemonic = Mnemonic::parse_in_normalized(Language::English, &mnemonic)
                    .map_err(|_| BuilderError::InvalidMnemonic)?;
                mnemonic.to_seed(passphrase)
            }
            SeedConfig::Bytes(bytes) => bytes,
        };
        self.seed_bytes = Some(seed);
        Ok(self)
    }

    /// Set the smallest change output the wallet's coin selection
    /// creates; smaller change goes to fees. Defaults to 25 000 sats.
    pub fn set_min_change_size(&mut self, min_change_size: u64) -> &mut Self {
        self.wallet_config.min_change_size = min_change_size;
        self
    }

    /// Set the logger for the DDK instance.
    pub fn set_logger(&mut self, logger: Arc<Logger>) -> &mut Self {
        self.logger = Some(logger);
        self
    }

    /// Set the hook consulted before a counterparty's cooperative close
    /// proposal is broadcast. Without one, every counterparty close proposal
    /// is rejected.
    pub fn set_cooperative_close_approver(
        &mut self,
        close_approver: Arc<dyn CooperativeCloseApprover>,
    ) -> &mut Self {
        self.close_approver = Some(close_approver);
        self
    }

    /// Setup the logger based on the provided logger or use default console logging
    fn setup_logger(&self, name: &str) -> Result<Arc<Logger>, Error> {
        match &self.logger {
            Some(logger) => Ok(logger.clone()),
            None => {
                // Default to console logging with Info level
                Ok(Arc::new(Logger::console(
                    name.to_string(),
                    DEFAULT_LOG_LEVEL,
                )))
            }
        }
    }

    /// Builds the `DlcDevKit` instance. Fails if any components are missing.
    #[tracing::instrument(name = "builder", skip(self))]
    pub async fn finish(&self) -> Result<DlcDevKit<T, S, O>, Error> {
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

        // Never fall back to a default seed: a forgotten seed must fail loudly,
        // not produce a wallet whose keys anyone can derive.
        let seed_bytes = self.seed_bytes.ok_or(BuilderError::NoSeed)?;

        let name = self
            .name
            .clone()
            .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());

        let logger = self.setup_logger(&name)?;

        let esplora_client = Arc::new(EsploraClient::new(
            &self.esplora_host,
            self.network,
            logger.clone(),
        )?);

        let wallet = Arc::new(
            DlcDevKitWallet::new_with_config(
                &seed_bytes,
                esplora_client.clone(),
                self.network,
                storage.clone(),
                self.contract_address_generator.clone(),
                self.wallet_config,
                logger.clone(),
            )
            .await?,
        );

        let mut oracles = HashMap::new();
        oracles.insert(oracle.get_public_key(), oracle.clone());

        let (sender, mut receiver) = tokio::sync::mpsc::channel(100);
        let (stop_signal_sender, stop_signal) = tokio::sync::watch::channel(false);

        let manager = Arc::new(
            Manager::new(
                wallet.clone(),
                wallet.clone(),
                esplora_client.clone(),
                storage.clone(),
                oracles,
                Arc::new(SystemTimeProvider {}),
                logger.clone(),
                self.close_approver.clone(),
            )
            .await?,
        );

        let manager_clone = manager.clone();
        let logger_clone = logger.clone();
        tokio::spawn(async move {
            // Every message runs in its own task: a panic in one request must
            // not take down the loop that also drives the periodic checks.
            while let Some(msg) = receiver.recv().await {
                match msg {
                    DlcManagerMessage::OfferDlc {
                        contract_input,
                        counter_party,
                        oracle_announcements,
                        responder,
                    } => {
                        let manager = manager_clone.clone();
                        let offer = tokio::spawn(async move {
                            let announcements =
                                group_announcements(&contract_input, oracle_announcements)?;
                            manager
                                .send_offer_with_announcements(
                                    &contract_input,
                                    counter_party,
                                    announcements,
                                )
                                .await
                        })
                        .await
                        .unwrap_or_else(|e| {
                            Err(ddk_manager::error::Error::InvalidState(format!(
                                "offer creation panicked: {e}"
                            )))
                        });

                        let _ = responder.send(offer).map_err(|e| {
                            log_error!(logger_clone.clone(), "Error sending offer: {:?}", e);
                        });
                    }
                    DlcManagerMessage::AcceptDlc {
                        contract,
                        responder,
                    } => {
                        let manager = manager_clone.clone();
                        let accept_dlc =
                            tokio::spawn(
                                async move { manager.accept_contract_offer(&contract).await },
                            )
                            .await
                            .unwrap_or_else(|e| {
                                Err(ddk_manager::error::Error::InvalidState(format!(
                                    "offer acceptance panicked: {e}"
                                )))
                            });

                        let _ = responder.send(accept_dlc).map_err(|e| {
                            log_error!(logger_clone.clone(), "Error sending accept DLC: {:?}", e);
                        });
                    }
                    DlcManagerMessage::PeriodicCheck => {
                        let manager = manager_clone.clone();
                        if let Err(e) =
                            tokio::spawn(async move { manager.periodic_check().await }).await
                        {
                            log_error!(logger_clone.clone(), "Periodic check panicked: {}", e);
                        }
                    }
                }
            }
        });

        let zmq_client = if let Some(endpoint) = &self.zmq_blockhash_endpoint {
            Some(Arc::new(
                ZeromqClient::new(endpoint, logger.clone(), stop_signal.clone()).await?,
            ))
        } else {
            None
        };

        log_info!(
            logger.clone(),
            "DDK runtime created. name={}, esplora={}, network={}, transport={}, oracle={}, zmq_enabled={}",
            name,
            self.esplora_host,
            self.network,
            transport.name(),
            oracle.get_public_key(),
            zmq_client.is_some()
        );

        Ok(DlcDevKit {
            runtime: Arc::new(RwLock::new(None)),
            wallet,
            manager,
            sender,
            transport,
            storage,
            oracle,
            network: self.network,
            stop_signal,
            stop_signal_sender,
            logger,
            zmq_client,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::oracle::memory::MemoryOracle;
    use crate::storage::memory::MemoryStorage;
    use crate::transport::memory::MemoryTransport;
    use bitcoin::secp256k1::Secp256k1;

    fn builder_without_seed() -> Builder<MemoryTransport, MemoryStorage, MemoryOracle> {
        let secp = Secp256k1::new();
        let logger = Arc::new(Logger::disabled("builder-test".to_string()));
        let mut builder = Builder::new();
        builder.set_network(Network::Regtest);
        builder.set_transport(Arc::new(MemoryTransport::new(&secp, logger.clone())));
        builder.set_storage(Arc::new(MemoryStorage::new()));
        builder.set_oracle(Arc::new(MemoryOracle::default()));
        builder.set_logger(logger);
        builder
    }

    #[tokio::test]
    async fn finish_without_a_seed_is_an_error() {
        let builder = builder_without_seed();
        let error = builder
            .finish()
            .await
            .err()
            .expect("a builder without a seed must not produce a wallet");
        assert!(
            matches!(error, Error::Builder(BuilderError::NoSeed)),
            "unexpected error: {error}"
        );
    }

    #[test]
    fn invalid_mnemonic_is_an_error() {
        let mut builder = builder_without_seed();
        let error = builder
            .set_seed_bytes(SeedConfig::Mnemonic(
                "not a mnemonic".to_string(),
                String::new(),
            ))
            .err()
            .expect("an invalid mnemonic must be rejected");
        assert!(matches!(error, BuilderError::InvalidMnemonic));
    }
}

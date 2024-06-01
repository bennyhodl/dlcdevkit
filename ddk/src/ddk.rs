pub use dlc_manager::Storage;
pub use dlc_messages::message_handler::MessageHandler as DlcMessageHandler;
pub use lightning_net_tokio;

use crate::chain::EsploraClient;
use crate::{get_dlc_dev_kit_dir, oracle::KormirOracleClient, wallet::DlcDevKitWallet, ORACLE_HOST};
use crate::{transport, DdkOracle, DdkStorage, DdkTransport};
use bdk::bitcoin::Network;
use bitcoin::secp256k1::{Parity, PublicKey, XOnlyPublicKey};
use dlc_manager::{
    contract::contract_input::ContractInput, manager::Manager, CachedContractSignerProvider,
    ContractId, Oracle, SimpleSigner, SystemTimeProvider,
};
use dlc_messages::{message_handler::MessageHandler, oracle_msgs::OracleAnnouncement};
use crate::storage::SledStorageProvider;
use crate::oracle::P2PDOracleClient;
use std::time::Duration;
use std::{
    collections::HashMap,
    sync::Arc,
};
use tokio::sync::Mutex;

pub type DlcDevKitDlcManager = dlc_manager::manager::Manager<
    Arc<DlcDevKitWallet>,
    Arc<CachedContractSignerProvider<Arc<DlcDevKitWallet>, SimpleSigner>>,
    Arc<EsploraClient>,
    Box<SledStorageProvider>,
    Box<P2PDOracleClient>,
    Arc<SystemTimeProvider>,
    Arc<DlcDevKitWallet>,
    SimpleSigner,
>;

pub struct DlcDevKit<T: DdkTransport, S: DdkStorage, O: DdkOracle> {
    pub wallet: Arc<DlcDevKitWallet>,
    pub manager: Arc<Mutex<DlcDevKitDlcManager>>,
    pub transport: Arc<T>,
    pub storage: Arc<S>,
    pub oracle: Arc<O>,
    // entropy (get seed from any source)
}

impl<T: DdkTransport + std::marker::Send + std::marker::Sync + 'static, S: DdkStorage, O: DdkOracle> DlcDevKit<T, S, O> {
    pub async fn new(
        name: &str,
        esplora_url: &str,
        network: Network,
        transport: Arc<T>,
        storage: Arc<S>,
        oracle: Arc<O>,
    ) -> anyhow::Result<DlcDevKit<T, S, O>> {
        log::info!("Creating new P2P DlcDevKit wallet. name={}", name);
        let wallet = Arc::new(DlcDevKitWallet::new(name, esplora_url, network)?);

        let db_path = get_dlc_dev_kit_dir().join(name);
        let dlc_storage = Box::new(SledStorageProvider::new(db_path.to_str().unwrap())?);

        let oracle_internal =
            tokio::task::spawn_blocking(move || P2PDOracleClient::new(ORACLE_HOST).unwrap())
                .await
                .unwrap();
        let mut oracles = HashMap::new();
        oracles.insert(oracle_internal.get_public_key(), Box::new(oracle_internal));

        let esplora_client = Arc::new(EsploraClient::new(esplora_url, network)?);

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

    pub async fn start(&self) -> anyhow::Result<()> {
        println!("Starting...");
        let transport_listener = self.transport.clone();
        let wallet = self.wallet.clone();
        let dlc_manager = self.manager.clone();

        tokio::spawn(async move {
            transport_listener.listen().await;
        });
        tokio::spawn(async move {
            let mut timer = tokio::time::interval(Duration::from_secs(10));
            loop {
                timer.tick().await;
                log::info!("Syncing wallet...");
                wallet.sync().unwrap();
            }
        });

        let transport_clone = self.transport.clone();
        tokio::spawn(async move {
            transport_clone.handle_dlc_message(&dlc_manager).await; 
        });

        Ok(())
    }

    pub async fn send_dlc_offer(
        &self,
        contract_input: &ContractInput,
        oracle_announcement: &OracleAnnouncement,
        counter_party: PublicKey,
    ) -> anyhow::Result<()> {
        let mut manager = self.manager.lock().await;

        let _offer_msg = manager.send_offer_with_announcements(
            contract_input,
            counter_party,
            vec![vec![oracle_announcement.clone()]],
        )?;

        Ok(())
    }

    pub async fn accept_dlc_offer(&self, contract: [u8; 32]) -> anyhow::Result<()> {
        let mut dlc = self.manager.lock().await;

        let contract_id = ContractId::from(contract);

        tracing::info!("Before accept: {:?}", contract_id);
        let (_, _public_key, _accept_dlc) = dlc.accept_contract_offer(&contract_id)?;

        tracing::info!("Accepted");

        Ok(())
    }
}

use crate::chain::EsploraClient;
use crate::wallet::DlcDevKitWallet;
use crate::{transport, DdkOracle, DdkStorage, DdkTransport};
use anyhow::anyhow;
use bdk::chain::PersistBackend;
use bdk::wallet::ChangeSet;
use bitcoin::secp256k1::PublicKey;
use bitcoin::Network;
use dlc_manager::{
    contract::contract_input::ContractInput, CachedContractSignerProvider, ContractId,
    SimpleSigner, SystemTimeProvider,
};
use dlc_messages::oracle_msgs::OracleAnnouncement;
use std::sync::{Arc, Mutex, RwLock};
use std::time::Duration;
use tokio::runtime::Runtime;

/// DlcDevKit type alias for the [dlc_manager::manager::Manager]
pub type DlcDevKitDlcManager<S, O> = dlc_manager::manager::Manager<
    Arc<DlcDevKitWallet>,
    Arc<CachedContractSignerProvider<Arc<DlcDevKitWallet>, SimpleSigner>>,
    Arc<EsploraClient>,
    Arc<S>,
    Arc<O>,
    Arc<SystemTimeProvider>,
    Arc<DlcDevKitWallet>,
    SimpleSigner,
>;

pub struct DlcDevKit<T: DdkTransport, S: DdkStorage, O: DdkOracle> {
    pub runtime: Arc<RwLock<Option<Runtime>>>,
    pub wallet: Arc<DlcDevKitWallet>,
    pub manager: Arc<Mutex<DlcDevKitDlcManager<S, O>>>,
    pub transport: Arc<T>,
    pub storage: Arc<S>,
    pub oracle: Arc<O>,
    pub network: Network,
}

impl<
        T: DdkTransport + std::marker::Send + std::marker::Sync + 'static,
        S: DdkStorage + std::marker::Send + std::marker::Sync + 'static,
        O: DdkOracle + std::marker::Send + std::marker::Sync + 'static,
    > DlcDevKit<T, S, O>
{
    pub fn start(&self) -> anyhow::Result<()> {
        tracing::info!("Starting ddk...");

        let mut runtime_lock = self.runtime.write().unwrap();

        if runtime_lock.is_some() {
            return Err(anyhow!("DDK is still running."));
        }

        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()?;
        // get fees

        let transport_clone = self.transport.clone();
        runtime.spawn(async move {
            tracing::info!("Starting listener");
            transport_clone.listen().await;
        });

        let wallet_clone = self.wallet.clone();
        runtime.spawn(async move {
            tracing::info!("started the wallet");
            let mut timer = tokio::time::interval(Duration::from_secs(10));
            loop {
                timer.tick().await;
                tracing::info!("Syncing wallet...");
                wallet_clone.sync().unwrap();
            }
        });

        let message_processor = self.transport.clone();
        let manager_clone = self.manager.clone();
        runtime.spawn(async move {
            tracing::info!("Message processor");
            let mut timer = tokio::time::interval(Duration::from_secs(5));
            loop {
                timer.tick().await;
                tracing::info!("Processing message...");
                process_incoming_messages(message_processor.clone(), manager_clone.clone());
            }
        });

        // TODO: connect stored peers.

        tracing::info!("DDK set up");
        *runtime_lock = Some(runtime);

        Ok(())
    }

    pub async fn send_dlc_offer(
        &self,
        contract_input: &ContractInput,
        oracle_announcement: &OracleAnnouncement,
        counter_party: PublicKey,
    ) -> anyhow::Result<()> {
        let mut manager = self.manager.lock().unwrap();

        let _offer_msg = manager.send_offer_with_announcements(
            contract_input,
            counter_party,
            vec![vec![oracle_announcement.clone()]],
        )?;

        Ok(())
    }

    pub fn network(&self) -> Network {
        self.network
    }

    pub fn accept_dlc_offer(&self, contract: [u8; 32]) -> anyhow::Result<()> {
        let mut dlc = self.manager.lock().unwrap();

        let contract_id = ContractId::from(contract);

        tracing::info!("Before accept: {:?}", contract_id);
        let (_, _public_key, _accept_dlc) = dlc.accept_contract_offer(&contract_id)?;

        tracing::info!("Accepted");

        Ok(())
    }
}

pub fn process_incoming_messages<T: DdkTransport, S: DdkStorage, O: DdkOracle>(
    transport: Arc<T>,
    manager: Arc<Mutex<DlcDevKitDlcManager<S, O>>>,
) {
    // let message_handler = self.transport.message_handler();
    // let peer_manager = self.transport.peer_manager();
    let messages = transport.get_and_clear_received_messages();

    for (counterparty, message) in messages {
        let resp = manager
            .lock()
            .unwrap()
            .on_dlc_message(&message, counterparty)
            .expect("Error processing message");

        if let Some(msg) = resp {
            transport.send_message(counterparty, msg);
        }
    }

    if transport.has_pending_messages() {
        tracing::info!("Still have pending messages!");
        // peer_manager.process_events();
    }
}

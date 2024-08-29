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
use dlc_messages::{AcceptDlc, Message, OfferDlc};
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
        let mut runtime_lock = self.runtime.write().unwrap();

        if runtime_lock.is_some() {
            return Err(anyhow!("DDK is still running."));
        }

        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()?;

        let transport_clone = self.transport.clone();
        runtime.spawn(async move {
            transport_clone.listen().await;
        });

        let wallet_clone = self.wallet.clone();
        runtime.spawn(async move {
            let mut timer = tokio::time::interval(Duration::from_secs(10));
            loop {
                timer.tick().await;
                wallet_clone.sync().unwrap();
            }
        });

        let message_processor = self.transport.clone();
        let manager_clone = self.manager.clone();
        runtime.spawn(async move {
            let mut timer = tokio::time::interval(Duration::from_secs(5));
            loop {
                timer.tick().await;
                process_incoming_messages(message_processor.clone(), manager_clone.clone(), || message_processor.process_messages());
            }
        });

        // TODO: connect stored peers.

        *runtime_lock = Some(runtime);

        Ok(())
    }

    pub fn connect_if_necessary(&self) -> anyhow::Result<()> {
        let _known_peers = self.storage.list_peers()?;
        
        // check from already connected
        
        Ok(()) 
    }

    pub fn send_dlc_offer(
        &self,
        contract_input: &ContractInput,
        counter_party: PublicKey,
        oracle_announcements: Vec<OracleAnnouncement>,
    ) -> anyhow::Result<OfferDlc> {
        let manager = self.manager.lock().unwrap();        

        let offer = manager.send_offer_with_announcements(
            contract_input,
            counter_party,
            vec![oracle_announcements]
        )?;

        let contract_id = hex::encode(&offer.temporary_contract_id);
        self.transport.send_message(counter_party, Message::Offer(offer.clone())); 
        tracing::info!(counterparty=counter_party.to_string(), contract_id, "Sent DLC offer to counterparty.");

        Ok(offer)
    }

    pub fn network(&self) -> Network {
        self.network
    }

    pub fn accept_dlc_offer(&self, contract: [u8; 32]) -> anyhow::Result<(String, String, AcceptDlc)> {
        let dlc = self.manager.lock().unwrap();

        let contract_id = ContractId::from(contract);

        let (contract_id, public_key, accept_dlc) = dlc.accept_contract_offer(&contract_id)?;

        self.transport
            .send_message(public_key, Message::Accept(accept_dlc.clone()));

        let contract_id = hex::encode(&contract_id);
        let counter_party = public_key.to_string();
        tracing::info!(counter_party, contract_id, "Accepted DLC contract.");

        Ok((contract_id, counter_party, accept_dlc))
    }
}

pub fn process_incoming_messages<T: DdkTransport, S: DdkStorage, O: DdkOracle, F: Fn() -> ()>(
    transport: Arc<T>,
    manager: Arc<Mutex<DlcDevKitDlcManager<S, O>>>,
    process_messages: F,
) {
    let messages = transport.get_and_clear_received_messages();

    for (counter_party, message) in messages {
        tracing::info!(counter_party=counter_party.to_string(), "Processing DLC message");
        let resp = manager
            .lock()
            .unwrap()
            .on_dlc_message(&message, counter_party)
            .expect("Error processing message");

        if let Some(msg) = resp {
            tracing::info!("Responding to message received.");
            tracing::debug!(message=?msg);
            transport.send_message(counter_party, msg);
        }
    }

    if transport.has_pending_messages() {
        process_messages()
    }
}

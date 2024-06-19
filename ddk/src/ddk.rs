use crate::chain::EsploraClient;
use crate::wallet::DlcDevKitWallet;
use crate::{transport, DdkOracle, DdkStorage, DdkTransport};
use anyhow::anyhow;
use bdk::chain::PersistBackend;
use bdk::wallet::ChangeSet;
use bitcoin::secp256k1::PublicKey;
use dlc_manager::{
    contract::contract_input::ContractInput, CachedContractSignerProvider, ContractId,
    SimpleSigner, SystemTimeProvider,
};
use dlc_messages::oracle_msgs::OracleAnnouncement;
use std::sync::{Arc, RwLock};
use std::time::Duration;
use tokio::runtime::Runtime;
use tokio::sync::Mutex;

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
}

impl<
        T: DdkTransport + std::marker::Send + std::marker::Sync + 'static,
        S: DdkStorage + std::marker::Send + std::marker::Sync + 'static,
        O: DdkOracle + std::marker::Send + std::marker::Sync + 'static,
    > DlcDevKit<T, S, O>
{
    pub fn start(&self) -> anyhow::Result<()> {
        println!("Starting ddk...");

        let mut runtime_lock = self.runtime.write().unwrap();

        if runtime_lock.is_some() {
            return Err(anyhow!("DDK is still running."));
        }

        let runtime = tokio::runtime::Builder::new_multi_thread().enable_all().build()?;
        // get fees

        let transport_clone = self.transport.clone();
        runtime.spawn(async move {
            tokio::runtime::Builder::new_current_thread()
                .build()
                .unwrap()
                .spawn(async move { transport_clone.listen().await })
        });

        let wallet_clone = self.wallet.clone();
        runtime.spawn(async move {
            println!("started the wallet");
            let mut timer = tokio::time::interval(Duration::from_secs(10));
            loop {
                timer.tick().await;
                println!("Syncing wallet...");
                wallet_clone.sync().unwrap();
            }
        });

        println!("Done starting ddk");
        *runtime_lock = Some(runtime);

        Ok(())
    }

    pub fn transport_type(&self) -> String {
        self.transport.name()
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

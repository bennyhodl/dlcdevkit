pub mod dlc;
pub mod peer_manager;
pub use lightning_net_tokio;
pub use peer_manager::ErnestPeerManager;

use bdk::bitcoin::Network;
use crate::{oracle::ErnestOracle, wallet::ErnestWallet, ORACLE_HOST};
use dlc_sled_storage_provider::SledStorageProvider;
use bitcoin::secp256k1::{Parity, XOnlyPublicKey, PublicKey};
use dlc_manager::{contract::contract_input::ContractInput, manager::Manager, ContractId, Oracle, CachedContractSignerProvider, SimpleSigner, SystemTimeProvider};
use dlc_messages::{message_handler::MessageHandler, oracle_msgs::OracleAnnouncement};
use p2pd_oracle_client::P2PDOracleClient;
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

pub type ErnestDlcManager = dlc_manager::manager::Manager<
    Arc<ErnestWallet>,
    Arc<CachedContractSignerProvider<Arc<ErnestWallet>, SimpleSigner>>,
    Arc<ErnestWallet>,
    Box<SledStorageProvider>,
    Box<P2PDOracleClient>,
    Arc<SystemTimeProvider>,
    Arc<ErnestWallet>,
    SimpleSigner,
>;

pub struct Ernest {
    pub wallet: Arc<ErnestWallet>,
    pub manager: Arc<Mutex<ErnestDlcManager>>,
    pub message_handler: Arc<MessageHandler>,
}

impl Ernest {
    pub async fn new(name: &str, esplora_url: &str, network: Network) -> anyhow::Result<Ernest> {
        let wallet = Arc::new(ErnestWallet::new(name, esplora_url, network)?);

        let dlc_storage = Box::new(SledStorageProvider::new(&name)?);

        // Ask carman!
        // let oracle = tokio::task::spawn_blocking(move || 
        //     Arc::new(ErnestOracle::new().unwrap())
        // ).await.unwrap();
        // let mut oracles = HashMap::new();
        // oracles.insert(oracle.get_public_key(), oracle);
        let oracle = tokio::task::spawn_blocking(move || 
            P2PDOracleClient::new(ORACLE_HOST).unwrap()
        ).await.unwrap();
        let mut oracles = HashMap::new();
        oracles.insert(oracle.get_public_key(), Box::new(oracle));

        let manager = Arc::new(Mutex::new(Manager::new(
            wallet.clone(),
            wallet.clone(),
            wallet.clone(),
            dlc_storage,
            oracles,
            Arc::new(SystemTimeProvider {}),
            wallet.clone(),
        )?));

        let message_handler = Arc::new(MessageHandler::new());

        Ok(Ernest {
            wallet,
            manager,
            message_handler,
        })
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

    pub async fn accept_dlc_offer(&self, contract: [u8; 32]) -> anyhow::Result<()> {
        let mut dlc = self.manager.lock().unwrap();

        let contract_id = ContractId::from(contract);

        let (_, public_key, _accept_dlc) = dlc.accept_contract_offer(&contract_id)?;

        let _xonly_pubkey =
            XOnlyPublicKey::from_slice(&public_key.x_only_public_key().0.serialize())?;

        Ok(())
    }
}

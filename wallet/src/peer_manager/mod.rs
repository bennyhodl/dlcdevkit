pub mod dlc;
pub mod peer_manager;

use crate::{dlc_storage::SledStorageProvider, oracle::ErnestOracle, wallet::ErnestWallet};
use bdk::bitcoin::secp256k1::PublicKey;
pub use bdk::bitcoin::Network;
use bitcoin::secp256k1::{Parity, XOnlyPublicKey};
pub use dlc_manager::SystemTimeProvider;
use dlc_manager::{contract::contract_input::ContractInput, manager::Manager, ContractId, Oracle};
use dlc_messages::{message_handler::MessageHandler, oracle_msgs::OracleAnnouncement};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

pub const RELAY_URL: &str = "ws://localhost:8081";

type ErnestOracles = HashMap<bdk::bitcoin::XOnlyPublicKey, ErnestOracle>;

pub type ErnestDlcManager = dlc_manager::manager::Manager<
    Arc<ErnestWallet>,
    Arc<ErnestWallet>,
    Arc<SledStorageProvider>,
    Arc<ErnestOracle>,
    Arc<SystemTimeProvider>,
    Arc<ErnestWallet>,
>;

pub struct Ernest {
    pub wallet: Arc<ErnestWallet>,
    pub manager: Arc<Mutex<ErnestDlcManager>>,
    pub message_handler: Arc<MessageHandler>,
}

impl Ernest {
    pub async fn new(name: &str, esplora_url: &str, network: Network) -> anyhow::Result<Ernest> {
        let wallet = Arc::new(ErnestWallet::new(name, esplora_url, network)?);

        let dlc_storage = Arc::new(SledStorageProvider::new(&name)?);

        let time = Arc::new(SystemTimeProvider {});

        // Ask carman!
        let oracle = Arc::new(ErnestOracle::new()?);
        let mut oracles = HashMap::new();
        oracles.insert(oracle.get_public_key(), oracle);

        let manager = Arc::new(Mutex::new(Manager::new(
            wallet.clone(),
            wallet.clone(),
            dlc_storage,
            oracles,
            time,
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
        xonly_pubkey: XOnlyPublicKey,
    ) -> anyhow::Result<()> {
        let pubkey = PublicKey::from_slice(&xonly_pubkey.public_key(Parity::Even).serialize())?;

        let mut manager = self.manager.lock().unwrap();

        let _offer_msg = manager.send_offer_with_announcements(
            contract_input,
            pubkey,
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

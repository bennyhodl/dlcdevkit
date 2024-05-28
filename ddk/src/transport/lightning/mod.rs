mod peer_manager;

// pub mod peer_manager;
// pub use dlc_manager::Storage;
// pub use dlc_messages::message_handler::MessageHandler as DlcMessageHandler;
// pub use lightning_net_tokio;
// pub use peer_manager::{DlcDevKitPeerManager, PeerManager};
//
// use crate::chain::EsploraClient;
// use crate::{get_dlc_dev_kit_dir, oracle::DlcDevKitOracle, wallet::DlcDevKitWallet, ORACLE_HOST};
// use bdk::bitcoin::Network;
// use bitcoin::secp256k1::{Parity, PublicKey, XOnlyPublicKey};
// use dlc_manager::{
//     contract::contract_input::ContractInput, manager::Manager, CachedContractSignerProvider,
//     ContractId, Oracle, SimpleSigner, SystemTimeProvider,
// };
// use dlc_messages::{message_handler::MessageHandler, oracle_msgs::OracleAnnouncement};
// use dlc_sled_storage_provider::SledStorageProvider;
// use p2pd_oracle_client::P2PDOracleClient;
// use std::{
//     collections::HashMap,
//     sync::{Arc, Mutex},
// };
//
// pub type DlcDevKitDlcManager = dlc_manager::manager::Manager<
//     Arc<DlcDevKitWallet>,
//     Arc<CachedContractSignerProvider<Arc<DlcDevKitWallet>, SimpleSigner>>,
//     Arc<EsploraClient>,
//     Box<SledStorageProvider>,
//     Box<P2PDOracleClient>,
//     Arc<SystemTimeProvider>,
//     Arc<DlcDevKitWallet>,
//     SimpleSigner,
// >;
//
// pub struct DlcDevKit {
//     pub wallet: Arc<DlcDevKitWallet>,
//     pub manager: Arc<Mutex<DlcDevKitDlcManager>>,
// }
//
// impl DlcDevKit {
//     pub async fn new(name: &str, esplora_url: &str, network: Network) -> anyhow::Result<DlcDevKit> {
//         log::info!("Creating new P2P DlcDevKit wallet. name={}", name);
//         let wallet = Arc::new(DlcDevKitWallet::new(name, esplora_url, network)?);
//
//         let db_path = get_dlc_dev_kit_dir().join(name);
//         let dlc_storage = Box::new(SledStorageProvider::new(db_path.to_str().unwrap())?);
//
//         // Ask carman!
//         // let oracle = tokio::task::spawn_blocking(move ||
//         //     Arc::new(DlcDevKitOracle::new().unwrap())
//         // ).await.unwrap();
//         // let mut oracles = HashMap::new();
//         // oracles.insert(oracle.get_public_key(), oracle);
//         let oracle =
//             tokio::task::spawn_blocking(move || P2PDOracleClient::new(ORACLE_HOST).unwrap())
//                 .await
//                 .unwrap();
//         let mut oracles = HashMap::new();
//         oracles.insert(oracle.get_public_key(), Box::new(oracle));
//
//         let esplora_client = Arc::new(EsploraClient::new(esplora_url, network)?);
//
//         let manager = Arc::new(Mutex::new(Manager::new(
//             wallet.clone(),
//             wallet.clone(),
//             esplora_client.clone(),
//             dlc_storage,
//             oracles,
//             Arc::new(SystemTimeProvider {}),
//             wallet.clone(),
//         )?));
//
//         Ok(DlcDevKit { wallet, manager })
//     }
//
//     pub async fn send_dlc_offer(
//         &self,
//         contract_input: &ContractInput,
//         oracle_announcement: &OracleAnnouncement,
//         counter_party: PublicKey,
//     ) -> anyhow::Result<()> {
//         let mut manager = self.manager.lock().unwrap();
//
//         let _offer_msg = manager.send_offer_with_announcements(
//             contract_input,
//             counter_party,
//             vec![vec![oracle_announcement.clone()]],
//         )?;
//
//         Ok(())
//     }
//
//     pub fn accept_dlc_offer(&self, contract: [u8; 32]) -> anyhow::Result<()> {
//         let mut dlc = self.manager.lock().unwrap();
//
//         let contract_id = ContractId::from(contract);
//
//         tracing::info!("Before accept: {:?}", contract_id);
//         let (_, _public_key, _accept_dlc) = dlc.accept_contract_offer(&contract_id)?;
//
//         tracing::info!("Accepted");
//
//         Ok(())
//     }
// }

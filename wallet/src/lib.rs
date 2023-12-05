#![allow(dead_code)]
#![allow(unused_imports)]
mod dlc;
mod wallet;
mod error;
mod io;
mod oracle;
mod sled;
mod nostr;

#[cfg(test)]
mod tests;

pub use bdk::bitcoin::Network;
use dlc_messages::message_handler::MessageHandler;
pub use crate::wallet::ErnestWallet;

use crate::{io::get_ernest_dir, oracle::Oracle as ErnestOracle, sled::SledStorageProvider, nostr::NostrDlcHandler};
use dlc_manager::SystemTimeProvider;
use std::{collections::HashMap, sync::{Arc, Mutex}};
use serde::Deserialize;

pub const RELAY_URL: &str = "http://localhost:8080";

pub type ErnestDlcManager = dlc_manager::manager::Manager<
    Arc<ErnestWallet>,
    Arc<ErnestWallet>,
    Arc<SledStorageProvider>,
    Arc<ErnestOracle>,
    Arc<SystemTimeProvider>,
    Arc<ErnestWallet>,
>;

// #[derive(Clone, Deserialize)]
pub struct Ernest {
    pub wallet: Arc<ErnestWallet>,
    pub manager: Arc<Mutex<ErnestDlcManager>>,
    pub message_handler: Arc<MessageHandler>,
    pub nostr: Arc<NostrDlcHandler>
}

impl Ernest {
    pub fn new(name: String, esplora_url: String, network: Network) -> anyhow::Result<Ernest> {
        let wallet = Arc::new(ErnestWallet::new(name.clone(), esplora_url, network)?);

        // TODO: Default path + config for storage
        let sled_path = get_ernest_dir().join(&name).join("dlc_db");

        let sled = Arc::new(SledStorageProvider::new(sled_path.to_str().unwrap())?);

        // let mut oracles: Arc<HashMap<XOnlyPublicKey, ErnestOracle>> = Arc::new(HashMap::new());
        // let oracle = ErnestOracle::default();
        // oracles.insert(oracle.get_public_key(), oracle);

        let time = Arc::new(SystemTimeProvider {});

        let manager = Arc::new(Mutex::new(dlc_manager::manager::Manager::new(
            wallet.clone(),
            wallet.clone(),
            sled,
            HashMap::new(),
            time,
            wallet.clone(),
        )?));

        let nostr = Arc::new(NostrDlcHandler::new(name, RELAY_URL.to_string(), manager.clone())?);

        let message_handler = Arc::new(MessageHandler::new());

        Ok(Ernest { wallet, manager, message_handler, nostr })
    }
}

pub mod dlc_handler;
pub mod relay_handler;

use crate::{wallet::ErnestWallet, RELAY_URL};
use bitcoin::Network;
use relay_handler::NostrDlcRelayHandler;
use std::sync::Arc;

pub struct ErnestNostr {
    pub wallet: Arc<ErnestWallet>,
    pub relays: Arc<NostrDlcRelayHandler>,
}

impl ErnestNostr {
    pub fn new(name: &str, esplora_url: &str, network: Network) -> anyhow::Result<ErnestNostr> {
        let wallet = Arc::new(ErnestWallet::new(name, esplora_url, network)?);

        let relays = Arc::new(NostrDlcRelayHandler::new(name, RELAY_URL.to_string())?);

        Ok(ErnestNostr { wallet, relays })
    }
}

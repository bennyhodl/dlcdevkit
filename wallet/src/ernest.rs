use crate::{oracle::Oracle, ErnestWallet, sled::SledStorageProvider, io::get_ernest_dir};
use bdk::bitcoin::Network;
use dlc_manager::SystemTimeProvider;
use std::{sync::Arc, collections::HashMap};

type ErnestDlcManager = dlc_manager::manager::Manager<
    Arc<ErnestWallet>,
    Arc<ErnestWallet>,
    Box<SledStorageProvider>,
    Box<HashMap<String, Oracle>>,
    Arc<SystemTimeProvider>,
    Arc<ErnestWallet>,
>;

pub struct Ernest {
    pub wallet: Arc<ErnestWallet>,
    pub manager: Arc<ErnestDlcManager>,
}

impl Ernest {
    pub fn new(name: String, esplora_url: String, network: Network) -> anyhow::Result<Ernest> {
        let wallet = Arc::new(ErnestWallet::new(name, esplora_url, network)?);

        let dir = get_ernest_dir();

        let sled = Box::new(SledStorageProvider::new(dir.join("sled").to_str().unwrap())?);

        let _oracle = Box::new(Oracle::default());

        let _oracles: HashMap<String, Oracle> = HashMap::new();

        let time = Arc::new(SystemTimeProvider {});

        let manager: ErnestDlcManager = dlc_manager::manager::Manager::new(
            wallet.clone(),
            wallet.clone(),
            sled,
            HashMap::new(),
            time,
            wallet.clone(),
        );

        Ok(Ernest { wallet, manager })
    }
}

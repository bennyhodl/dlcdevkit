use crate::{io::get_ernest_dir, oracle::Oracle as ErnestOracle, sled::SledStorageProvider, ErnestWallet};
use bdk::bitcoin::Network;
use dlc_manager::SystemTimeProvider;
use std::{collections::HashMap, sync::Arc};

type ErnestDlcManager = dlc_manager::manager::Manager<
    Arc<ErnestWallet>,
    Arc<ErnestWallet>,
    Arc<SledStorageProvider>,
    Arc<ErnestOracle>,
    Arc<SystemTimeProvider>,
    Arc<ErnestWallet>,
>;

pub struct Ernest {
    pub wallet: Arc<ErnestWallet>,
    pub manager: Arc<ErnestDlcManager>,
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

        let manager: ErnestDlcManager = dlc_manager::manager::Manager::new(
            wallet.clone(),
            wallet.clone(),
            sled,
            HashMap::new(),
            time,
            wallet.clone(),
        )?;

        Ok(Ernest { wallet, manager: Arc::new(manager) })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use electrsd::{ElectrsD, bitcoind::BitcoinD};

    #[test]
    fn create_manager() {
        let bitcoind = electrsd::bitcoind::downloaded_exe_path().expect("No link?");
        let mut bitcoind_conf = electrsd::bitcoind::Conf::default();
        bitcoind_conf.network = "regtest";
        let bitcoind = BitcoinD::with_conf(bitcoind, &bitcoind_conf).unwrap();

        let electrs_exe = electrsd::downloaded_exe_path()
            .expect("you need to provide env var ELECTRS_EXE or specify an electrsd version feature");
        let mut electrsd_conf = electrsd::Conf::default();
        electrsd_conf.http_enabled = true;
        electrsd_conf.network = "regtest";
        let electrsd = ElectrsD::with_conf(electrs_exe, &bitcoind, &electrsd_conf).unwrap();

        let esplora_url = format!("http://{}", electrsd.esplora_url.as_ref().unwrap());

        let manager = Ernest::new("test".to_string(), esplora_url.to_string(), Network::Regtest);

        assert_eq!(manager.is_ok(), true)

    }
}

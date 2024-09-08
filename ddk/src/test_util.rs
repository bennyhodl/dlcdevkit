use bitcoin::{bip32::Xpriv, key::rand::Fill, Network};
use dlc_manager::{manager::Manager, SystemTimeProvider};
use std::sync::Arc;

use crate::{
    chain::EsploraClient, oracle::P2PDOracleClient, storage::SledStorageProvider,
    wallet::DlcDevKitWallet,
};

type TestManager = Arc<
    Manager<
        Arc<DlcDevKitWallet<SledStorageProvider>>,
        Arc<
            dlc_manager::CachedContractSignerProvider<
                Arc<DlcDevKitWallet<SledStorageProvider>>,
                dlc_manager::SimpleSigner,
            >,
        >,
        Arc<EsploraClient>,
        Arc<SledStorageProvider>,
        Arc<P2PDOracleClient>,
        Arc<SystemTimeProvider>,
        Arc<DlcDevKitWallet<SledStorageProvider>>,
        dlc_manager::SimpleSigner,
    >,
>;

pub struct TestWallet {
    pub wallet: DlcDevKitWallet<SledStorageProvider>,
    pub path: String,
}

impl TestWallet {
    pub fn create_wallet(name: &str) -> TestWallet {
        let path = format!("tests/data/{name}");
        let storage = Arc::new(SledStorageProvider::new(&path).unwrap());
        let mut entropy = [0u8; 64];
        entropy
            .try_fill(&mut bitcoin::key::rand::thread_rng())
            .unwrap();
        let xpriv = Xpriv::new_master(Network::Regtest, &entropy).unwrap();
        let wallet = DlcDevKitWallet::new(
            "test".into(),
            xpriv,
            "http://localhost:30000",
            Network::Regtest,
            &path,
            storage.clone(),
        )
        .unwrap();
        TestWallet { wallet, path }
    }
}

impl Drop for TestWallet {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.path).expect("Couldn't remove wallet dir");
    }
}

use crate::ErnestWallet;
use bdk::bitcoin::Network;
use std::{
    sync::{Arc, RwLock},
    time::Duration,
};
use tokio::time::MissedTickBehavior;

pub type ErnestRuntime = Arc<RwLock<Option<tokio::runtime::Runtime>>>;

pub struct Ernest {
    pub runtime: ErnestRuntime,
    pub wallet: Arc<ErnestWallet>,
}

impl Ernest {
    pub fn start(&self) {
        let mut runtime_lock = self.runtime.write().unwrap();

        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .unwrap();

        let wallet = self.wallet.clone();

        std::thread::spawn(move || {
            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap()
                .block_on(async move {
                    let mut sync_interval = tokio::time::interval(Duration::from_secs(10));
                    sync_interval.set_missed_tick_behavior(MissedTickBehavior::Skip);

                    loop {
                        tokio::select! {
                            _ = sync_interval.tick() => {
                                println!("Syncing to chain.");
                                let _ = wallet.sync().await;
                            }
                        }
                    }
                })
        });

        *runtime_lock = Some(runtime);
    }
}

pub fn build(name: String, esplora_url: String, network: Network) -> anyhow::Result<Ernest> {
    let runtime = Arc::new(RwLock::new(None));

    let wallet = Arc::new(ErnestWallet::new(
        name,
        esplora_url,
        network,
        runtime.clone(),
    )?);

    Ok(Ernest { runtime, wallet })
}

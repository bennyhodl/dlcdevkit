```
# BDK 1.0
let secp = Secp256k1::new();
let bip84_external = DerivationPath::from_str("m/84'/1'/0'/0/0")?;
let bip84_internal = DerivationPath::from_str("m/84'/1'/0'/0/1")?;

let external_key = (privkey, bip84_external).into_descriptor_key()?;
let internal_key = (privkey, bip84_internal).into_descriptor_key()?;

let external_descriptor =
    descriptor!(wpkh(external_key))?.into_wallet_descriptor(&secp, network)?;
let internal_descriptor =
    descriptor!(wpkh(internal_key))?.into_wallet_descriptor(&secp, network)?;

let chain_file = match File::open(DB_CHAIN_STORE) {
    Ok(file) => file,
    Err(_) => {
        File::create(DB_CHAIN_STORE)?
    }
};

let db = Store::<bdk::wallet::ChangeSet>::new(DB_MAGIC, chain_file)?;

let wallet =
    Wallet::new(external_descriptor, Some(internal_descriptor), db, network)?;

```

```
# Party params!

use crate::ErnestWallet;
use dlc::PartyParams;

impl ErnestWallet {
    pub async fn create_party_params(
        &self,
        input_amount: u64,
        collateral: u64,
    ) -> anyhow::Result<PartyParams> {
        let fund_pubkey = self.get_pubkey()?;

        let change_script_pubkey = self.new_change_address()?;
        let payout_script_pubkey = self.new_external_address()?;

        // Inputs? Need to select coins that equal the input amount/collateral

        let party_params = PartyParams {
            fund_pubkey,
            change_script_pubkey: change_script_pubkey.script_pubkey(),
            payout_script_pubkey: payout_script_pubkey.script_pubkey(),
            change_serial_id: 0,
            payout_serial_id: 0,
            inputs: Vec::new(),
            input_amount,
            collateral,
        };
        Ok(party_params)
    }
}

#[cfg(test)]
mod dlc_tests {
    use crate::tests::util::setup_bitcoind_and_electrsd_and_ernest_wallet;
    #[tokio::test]
    async fn test_party_params() {
        let (_, _, wallet) = setup_bitcoind_and_electrsd_and_ernest_wallet();

        let party_params = wallet.create_party_params(10, 50).await;

        assert_eq!(party_params.is_ok(), true)
    }
}

```

```
# Runtime bullshit

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
```

```
let wallet_name = bdk::wallet::wallet_name_from_descriptor(
    Bip84(xprv, KeychainKind::External),
    Some(Bip84(xprv, KeychainKind::Internal)),
    network,
    &Secp256k1::new(),
)?;
```

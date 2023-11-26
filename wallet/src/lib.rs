mod dlc;
#[cfg(test)]
mod tests;
pub use bdk::bitcoin::Network;
use bdk::{
    blockchain::EsploraBlockchain,
    database::SqliteDatabase,
    template::Bip84,
    wallet::{AddressIndex, AddressInfo},
    Balance, KeychainKind, Wallet,
};
use bitcoin::{
    secp256k1::{PublicKey, Secp256k1},
    util::bip32::ExtendedPrivKey,
};
use std::sync::{Arc, RwLock};
mod io;

use io::{create_ernest_dir_with_wallet, get_wallet_dir};

#[derive(Debug)]
pub struct ErnestWallet {
    pub blockchain: EsploraBlockchain,
    pub wallet: Arc<RwLock<Wallet<SqliteDatabase>>>,
    pub name: String,
}

impl ErnestWallet {
    pub fn new(
        wallet_name: String,
        esplora_url: String,
        network: Network,
    ) -> anyhow::Result<ErnestWallet> {
        let wallet_dir = create_ernest_dir_with_wallet(wallet_name.clone())?;

        // Save the seed to the OS keychain. Not in home directory.
        let ernest_dir = wallet_dir
            .clone()
            .parent()
            .unwrap()
            .join(format!("{}_seed", wallet_name));
        let seed = io::read_or_generate_seed(ernest_dir)?;

        let privkey = ExtendedPrivKey::new_master(network, &seed)?;

        let _wallet_name = bdk::wallet::wallet_name_from_descriptor(
            Bip84(privkey, KeychainKind::External),
            Some(Bip84(privkey, KeychainKind::Internal)),
            network,
            &Secp256k1::new(),
        )?;

        let database = SqliteDatabase::new(wallet_dir);

        let wallet = Wallet::new(
            Bip84(privkey, KeychainKind::External),
            Some(Bip84(privkey, KeychainKind::Internal)),
            network,
            database,
        )?;

        let blockchain = EsploraBlockchain::new(&esplora_url, 20).with_concurrency(4);

        Ok(ErnestWallet {
            blockchain,
            wallet: Arc::new(RwLock::new(wallet)),
            name: wallet_name,
        })
    }

    pub fn get_pubkey(&self) -> anyhow::Result<PublicKey> {
        let dir = get_wallet_dir(self.name.clone());
        let seed = std::fs::read_to_string(dir.join(format!("{}_seed", self.name.clone())))?;

        let pubkey = PublicKey::from_slice(&seed.as_bytes())?;

        Ok(pubkey)
    }

    pub async fn get_balance(&self) -> anyhow::Result<Balance> {
        self.wallet
            .read()
            .unwrap()
            .sync(&self.blockchain, bdk::SyncOptions { progress: None })
            .await?;

        let balance = self.wallet.try_read().unwrap().get_balance()?;

        Ok(balance)
    }

    pub fn new_external_address(&self) -> anyhow::Result<AddressInfo> {
        let address = self
            .wallet
            .try_write()
            .unwrap()
            .get_address(AddressIndex::New)?;

        Ok(address)
    }

    pub fn new_change_address(&self) -> anyhow::Result<AddressInfo> {
        let address = self
            .wallet
            .try_write()
            .unwrap()
            .get_internal_address(AddressIndex::New)?;

        Ok(address)
    }
}

// BDK 1.0
// let secp = Secp256k1::new();
// let bip84_external = DerivationPath::from_str("m/84'/1'/0'/0/0")?;
// let bip84_internal = DerivationPath::from_str("m/84'/1'/0'/0/1")?;
//
// let external_key = (privkey, bip84_external).into_descriptor_key()?;
// let internal_key = (privkey, bip84_internal).into_descriptor_key()?;
//
// let external_descriptor =
//     descriptor!(wpkh(external_key))?.into_wallet_descriptor(&secp, network)?;
// let internal_descriptor =
//     descriptor!(wpkh(internal_key))?.into_wallet_descriptor(&secp, network)?;
//
// let chain_file = match File::open(DB_CHAIN_STORE) {
//     Ok(file) => file,
//     Err(_) => {
//         File::create(DB_CHAIN_STORE)?
//     }
// };
//
// let db = Store::<bdk::wallet::ChangeSet>::new(DB_MAGIC, chain_file)?;
//
// let wallet =
//     Wallet::new(external_descriptor, Some(internal_descriptor), db, network)?;

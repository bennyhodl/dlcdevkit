use bdk::{
    bitcoin::Network,
    blockchain::EsploraBlockchain,
    database::SqliteDatabase,
    template::Bip84,
    wallet::{AddressIndex, AddressInfo},
    Balance, KeychainKind, Wallet,
};
use bip39::Mnemonic;
use bitcoin::{secp256k1::Secp256k1, util::bip32::ExtendedPrivKey};
use getrandom::getrandom;
use std::sync::{Arc, RwLock};

#[derive(Debug)]
pub struct ErnestWallet {
    pub blockchain: EsploraBlockchain,
    pub wallet: Arc<RwLock<Wallet<SqliteDatabase>>>,
}

impl ErnestWallet {
    pub fn new(network: Network) -> anyhow::Result<ErnestWallet> {
        let mut entropy = [0u8; 16];

        getrandom(&mut entropy)?;

        let _mnemonic = Mnemonic::from_entropy(&entropy)?;

        let privkey = ExtendedPrivKey::new_master(network, &entropy)?;

        let wallet_name = bdk::wallet::wallet_name_from_descriptor(
            Bip84(privkey, KeychainKind::External),
            Some(Bip84(privkey, KeychainKind::Internal)),
            network,
            &Secp256k1::new(),
        )?;

        let db_filename = format!("./wallets/{}_ernest.sqlite", wallet_name);
        let database = SqliteDatabase::new(db_filename);

        let wallet = Wallet::new(
            Bip84(privkey, KeychainKind::External),
            Some(Bip84(privkey, KeychainKind::Internal)),
            network,
            database,
        )?;

        let blockchain =
            EsploraBlockchain::new("https://mutinynet.com/api/v1", 20).with_concurrency(4);

        Ok(ErnestWallet {
            blockchain,
            wallet: Arc::new(RwLock::new(wallet)),
        })
    }

    pub fn get_balance(&self) -> anyhow::Result<Balance> {
        let balance = self.wallet.try_read().unwrap().get_balance()?;

        Ok(balance)
    }

    pub fn new_address(&self) -> anyhow::Result<AddressInfo> {
        let address = self
            .wallet
            .try_write()
            .unwrap()
            .get_address(AddressIndex::New)?;

        Ok(address)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn create_wallet() {
        let wallet = ErnestWallet::new(Network::Regtest);

        assert_eq!(wallet.is_ok(), true)
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

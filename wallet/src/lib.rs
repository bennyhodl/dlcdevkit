mod dlc;
mod ernest;
mod io;

#[cfg(test)]
mod tests;

pub use bdk::bitcoin::Network;
pub use ernest::{build, ErnestRuntime};

use anyhow::anyhow;
use bdk::{
    bitcoin::{
        secp256k1::{PublicKey, Secp256k1},
        Address, Txid,
    },
    blockchain::EsploraBlockchain,
    database::SqliteDatabase,
    template::Bip84,
    wallet::{AddressIndex, AddressInfo},
    Balance, FeeRate, KeychainKind, SignOptions, SyncOptions, Wallet,
};
use bitcoin::util::bip32::{ExtendedPrivKey, ExtendedPubKey};
use std::sync::Mutex;
use io::{create_ernest_dir_with_wallet, get_ernest_dir};

#[derive(Debug)]
pub struct ErnestWallet {
    pub blockchain: EsploraBlockchain,
    pub inner: Mutex<Wallet<SqliteDatabase>>,
    pub name: String,
    pub runtime: ErnestRuntime,
}

impl ErnestWallet {
    pub fn new(
        name: String,
        esplora_url: String,
        network: Network,
        runtime: ErnestRuntime,
    ) -> anyhow::Result<ErnestWallet> {
        let wallet_dir = create_ernest_dir_with_wallet(name.clone())?;

        // Save the seed to the OS keychain. Not in home directory.
        let ernest_dir = wallet_dir
            .clone()
            .parent()
            .unwrap()
            .join(format!("{}_seed", name));

        let xprv = io::read_or_generate_xprv(ernest_dir, network)?;

        let _wallet_name = bdk::wallet::wallet_name_from_descriptor(
            Bip84(xprv, KeychainKind::External),
            Some(Bip84(xprv, KeychainKind::Internal)),
            network,
            &Secp256k1::new(),
        )?;

        let database = SqliteDatabase::new(wallet_dir);

        let inner = Mutex::new(Wallet::new(
            Bip84(xprv, KeychainKind::External),
            Some(Bip84(xprv, KeychainKind::Internal)),
            network,
            database,
        )?);

        let blockchain = EsploraBlockchain::new(&esplora_url, 20).with_concurrency(4);

        Ok(ErnestWallet {
            blockchain,
            inner,
            name,
            runtime,
        })
    }

    pub async fn sync(&self) -> anyhow::Result<()> {
        let wallet_lock = self.inner.lock().unwrap();
        let sync_opts = SyncOptions { progress: None };
        let sync = wallet_lock.sync(&self.blockchain, sync_opts).await?;
        Ok(sync)
    }

    pub fn get_pubkey(&self) -> anyhow::Result<PublicKey> {
        let dir = get_ernest_dir();

        let seed = std::fs::read(dir.join(format!("{}_seed", self.name.clone())))?;

        let slice = seed.as_slice();

        let xprv = ExtendedPrivKey::decode(slice)?;

        let secp = Secp256k1::new();

        let pubkey = ExtendedPubKey::from_priv(&secp, &xprv);

        Ok(pubkey.public_key)
    }

    pub async fn get_balance(&self) -> anyhow::Result<Balance> {
        let guard = self.inner.lock().unwrap();
        guard
            .sync(&self.blockchain, bdk::SyncOptions { progress: None })
            .await?;

        let balance = guard.get_balance()?;

        Ok(balance)
    }

    pub fn new_external_address(&self) -> anyhow::Result<AddressInfo> {
        let guard = self.inner.lock().unwrap();
        let address = guard.get_address(AddressIndex::New)?;

        Ok(address)
    }

    pub fn new_change_address(&self) -> anyhow::Result<AddressInfo> {
        let guard = self.inner.lock().unwrap();
        let address = guard.get_internal_address(AddressIndex::New)?;

        Ok(address)
    }

    pub async fn send_to_address(
        &self,
        address: Address,
        amount: u64,
        sat_vbyte: f32,
    ) -> anyhow::Result<Txid> {
        let guard = self.inner.lock().unwrap();

        let mut txn_builder = guard.build_tx();

        txn_builder
            .add_recipient(address.script_pubkey(), amount)
            .fee_rate(FeeRate::from_sat_per_vb(sat_vbyte));

        let (mut psbt, _details) = txn_builder.finish()?;

        guard.sign(&mut psbt, SignOptions::default())?;

        let tx = psbt.extract_tx();

        match self.blockchain.broadcast(&tx).await {
            Ok(_) => ..,
            Err(e) => return Err(anyhow!("Could not broadcast txn {}", e)),
        };

        Ok(tx.txid())
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

#![allow(dead_code)]
mod dlc;
mod ernest;
mod error;
mod io;
mod oracle;
mod sled;

#[cfg(test)]
mod tests;

pub use bdk::bitcoin::Network;

use anyhow::anyhow;
use bdk::{
    bitcoin::{
        secp256k1::{PublicKey, Secp256k1},
        util::bip32::{ExtendedPrivKey, ExtendedPubKey},
        Address, Txid,
    },
    blockchain::EsploraBlockchain,
    database::SqliteDatabase,
    template::Bip84,
    wallet::{AddressIndex, AddressInfo},
    Balance, FeeRate, KeychainKind, SignOptions, SyncOptions, Wallet,
};
use io::get_ernest_dir;
use lightning::chain::chaininterface::ConfirmationTarget;
use std::sync::atomic::AtomicU32;
use std::sync::Arc;
use std::{collections::HashMap, sync::Mutex};

pub struct ErnestWallet {
    pub blockchain: EsploraBlockchain,
    pub inner: Mutex<Wallet<SqliteDatabase>>,
    pub network: Network,
    pub xprv: ExtendedPrivKey,
    pub name: String,
    fees: Arc<HashMap<ConfirmationTarget, AtomicU32>>,
}

const MIN_FEERATE: u32 = 253;

impl ErnestWallet {
    pub fn new(
        name: String,
        esplora_url: String,
        network: Network,
    ) -> anyhow::Result<ErnestWallet> {
        let xprv = io::read_or_generate_xprv(name.clone(), network)?;

        let db_path = get_ernest_dir().join(&name).join("wallet_db");

        let database = SqliteDatabase::new(db_path);

        let inner = Mutex::new(Wallet::new(
            Bip84(xprv, KeychainKind::External),
            Some(Bip84(xprv, KeychainKind::Internal)),
            network,
            database,
        )?);

        let blockchain = EsploraBlockchain::new(&esplora_url, 20).with_concurrency(4);

        let mut fees: HashMap<ConfirmationTarget, AtomicU32> = HashMap::new();
        fees.insert(ConfirmationTarget::OnChainSweep, AtomicU32::new(5000));
        fees.insert(
            ConfirmationTarget::MaxAllowedNonAnchorChannelRemoteFee,
            AtomicU32::new(25 * 250),
        );
        fees.insert(
            ConfirmationTarget::MinAllowedAnchorChannelRemoteFee,
            AtomicU32::new(MIN_FEERATE),
        );
        fees.insert(
            ConfirmationTarget::MinAllowedNonAnchorChannelRemoteFee,
            AtomicU32::new(MIN_FEERATE),
        );
        fees.insert(
            ConfirmationTarget::AnchorChannelFee,
            AtomicU32::new(MIN_FEERATE),
        );
        fees.insert(
            ConfirmationTarget::NonAnchorChannelFee,
            AtomicU32::new(2000),
        );
        fees.insert(
            ConfirmationTarget::ChannelCloseMinimum,
            AtomicU32::new(MIN_FEERATE),
        );
        let fees = Arc::new(fees);

        Ok(ErnestWallet {
            blockchain,
            inner,
            network,
            xprv,
            name,
            fees,
        })
    }

    pub fn sync(&self) -> anyhow::Result<()> {
        let wallet_lock = self.inner.lock().unwrap();
        let sync_opts = SyncOptions { progress: None };
        let sync = wallet_lock.sync(&self.blockchain, sync_opts)?;
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

    pub fn get_balance(&self) -> anyhow::Result<Balance> {
        let guard = self.inner.lock().unwrap();

        guard.sync(&self.blockchain, bdk::SyncOptions { progress: None })?;

        let balance = guard.get_balance()?;

        Ok(balance)
    }

    pub fn new_external_address(&self) -> Result<AddressInfo, bdk::Error> {
        let guard = self.inner.lock().unwrap();
        let address = guard.get_address(AddressIndex::New)?;

        Ok(address)
    }

    pub fn new_change_address(&self) -> anyhow::Result<AddressInfo> {
        let guard = self.inner.lock().unwrap();
        let address = guard.get_internal_address(AddressIndex::New)?;

        Ok(address)
    }

    pub fn send_to_address(
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

        match self.blockchain.broadcast(&tx) {
            Ok(_) => ..,
            Err(e) => return Err(anyhow!("Could not broadcast txn {}", e)),
        };

        Ok(tx.txid())
    }
}
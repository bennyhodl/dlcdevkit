use crate::io;
use anyhow::anyhow;
use bdk::{
    bitcoin::{
        bip32::{ExtendedPrivKey, ExtendedPubKey},
        key::{KeyPair, XOnlyPublicKey},
        secp256k1::{All, PublicKey, Secp256k1},
        Address, Network, Txid,
    },
    blockchain::EsploraBlockchain,
    template::Bip86,
    wallet::{AddressIndex, AddressInfo},
    Balance, FeeRate, KeychainKind, SignOptions, SyncOptions, Wallet,
};
use lightning::chain::chaininterface::ConfirmationTarget;
use serde::Deserialize;
use sled::Tree;
use std::sync::Arc;
use std::sync::{atomic::AtomicU32, RwLock};
use std::{collections::HashMap, sync::Mutex};

const SLED_TREE: &str = "bdk_store";

pub struct ErnestWallet {
    pub blockchain: Arc<EsploraBlockchain>,
    pub inner: Arc<Mutex<Wallet<Tree>>>,
    pub network: Network,
    pub xprv: ExtendedPrivKey,
    pub name: String,
    secp: Secp256k1<All>,
}

const MIN_FEERATE: u32 = 253;

impl ErnestWallet {
    pub fn new(name: &str, esplora_url: &str, network: Network) -> anyhow::Result<ErnestWallet> {
        let secp = Secp256k1::new();
        let xprv = io::read_or_generate_xprv(name, network)?;

        let db_path = io::get_ernest_dir().join(&name).join("wallet_db");

        let sled = sled::open(db_path)?.open_tree(SLED_TREE)?;

        let inner = Arc::new(Mutex::new(Wallet::new(
            Bip86(xprv, KeychainKind::External),
            Some(Bip86(xprv, KeychainKind::Internal)),
            network,
            sled,
        )?));

        let blockchain = Arc::new(EsploraBlockchain::new(&esplora_url, 20).with_concurrency(4));

        let mut fees: HashMap<ConfirmationTarget, AtomicU32> = HashMap::new();
        fees.insert(ConfirmationTarget::OnChainSweep, AtomicU32::new(5000));
        fees.insert(
            ConfirmationTarget::MinAllowedAnchorChannelRemoteFee,
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
        let _fees = Arc::new(fees);

        Ok(ErnestWallet {
            blockchain,
            inner,
            network,
            xprv,
            secp,
            name: name.to_string(),
        })
    }

    pub fn sync(&self) -> anyhow::Result<()> {
        let wallet_lock = self.inner.lock().unwrap();
        let sync_opts = SyncOptions { progress: None };
        let sync = wallet_lock.sync(&self.blockchain, sync_opts)?;
        Ok(sync)
    }

    pub fn get_pubkey(&self) -> anyhow::Result<PublicKey> {
        let pubkey = PublicKey::from_secret_key(&self.secp, &self.xprv.private_key);
        Ok(pubkey)
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

    pub fn new_change_address(&self) -> Result<AddressInfo, bdk::Error> {
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

use crate::{
    chain::EsploraClient,
    signer::{DeriveSigner, SimpleDeriveSigner},
    storage::SledStorageProvider,
    DdkConfig,
};
use anyhow::anyhow;
use bdk::{
    bitcoin::{
        bip32::{DerivationPath, ExtendedPrivKey},
        secp256k1::{All, PublicKey, Secp256k1},
        Address, Network, Txid,
    },
    chain::PersistBackend,
    template::Bip86,
    wallet::{AddressIndex, AddressInfo, Balance, ChangeSet, Update},
    KeychainKind, SignOptions, Wallet,
};
use bdk_esplora::EsploraExt;
use bdk_file_store::Store;
use bitcoin::{FeeRate, ScriptBuf};
use blake3::Hasher;
use dlc_manager::{error::Error as ManagerError, SimpleSigner};
use lightning::chain::chaininterface::{ConfirmationTarget, FeeEstimator};
use std::sync::{atomic::Ordering, Arc};
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::Mutex,
};
use std::{str::FromStr, sync::atomic::AtomicU32};

pub struct DlcDevKitWallet {
    pub blockchain: Arc<EsploraClient>,
    pub inner: Arc<Mutex<Wallet<Store<ChangeSet>>>>,
    pub network: Network,
    pub xprv: ExtendedPrivKey,
    pub name: String,
    pub fees: Arc<HashMap<ConfirmationTarget, AtomicU32>>,
    simple_derive_signer: SimpleDeriveSigner,
    secp: Secp256k1<All>,
}

const MIN_FEERATE: u32 = 253;

impl DlcDevKitWallet {
    pub fn new<P>(
        name: &str,
        xprv: ExtendedPrivKey,
        esplora_url: &str,
        network: Network,
        wallet_storage_path: P,
    ) -> anyhow::Result<DlcDevKitWallet>
    where
        P: AsRef<Path>,
    {
        let secp = Secp256k1::new();
        let wallet_storage_path = wallet_storage_path.as_ref().join("ddk-wallet");
        let storage = Store::<ChangeSet>::open_or_create_new(&[0u8; 32], wallet_storage_path)?;

        let inner = Arc::new(Mutex::new(Wallet::new_or_load(
            Bip86(xprv, KeychainKind::External),
            Some(Bip86(xprv, KeychainKind::Internal)),
            storage,
            network,
        )?));

        let blockchain = Arc::new(EsploraClient::new(esplora_url, network)?);

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
        let fees = Arc::new(fees);

        Ok(DlcDevKitWallet {
            blockchain,
            inner,
            network,
            xprv,
            fees,
            simple_derive_signer: SimpleDeriveSigner {},
            secp,
            name: name.to_string(),
        })
    }

    pub fn sync(&self) -> anyhow::Result<()> {
        let mut wallet = self.inner.lock().unwrap();
        let prev_tip = wallet.latest_checkpoint();
        let keychain_spks = wallet.all_unbounded_spk_iters().into_iter().collect();
        let (update_graph, last_active_indices) =
            self.blockchain
                .blocking_client
                .full_scan(keychain_spks, 5, 1)?;
        let missing_height = update_graph.missing_heights(wallet.local_chain());
        let chain_update = self
            .blockchain
            .blocking_client
            .update_local_chain(prev_tip, missing_height)?;
        let update = Update {
            last_active_indices,
            graph: update_graph,
            chain: Some(chain_update),
        };

        wallet.apply_update(update)?;
        wallet.commit().unwrap();
        Ok(())
    }

    pub fn get_pubkey(&self) -> anyhow::Result<PublicKey> {
        let pubkey = PublicKey::from_secret_key(&self.secp, &self.xprv.private_key);
        Ok(pubkey)
    }

    pub fn get_balance(&self) -> anyhow::Result<Balance> {
        let guard = self.inner.lock().unwrap();
        let balance = guard.get_balance();

        Ok(balance)
    }

    pub fn new_external_address(&self) -> anyhow::Result<AddressInfo> {
        let mut guard = self.inner.lock().unwrap();
        let address = guard.try_get_address(AddressIndex::New).unwrap();

        Ok(address)
    }

    pub fn new_change_address(&self) -> anyhow::Result<AddressInfo> {
        let mut guard = self.inner.lock().unwrap();
        let address = guard.try_get_internal_address(AddressIndex::New).unwrap();

        Ok(address)
    }

    pub fn send_to_address(
        &self,
        address: Address,
        amount: u64,
        sat_vbyte: u64,
    ) -> anyhow::Result<Txid> {
        let mut guard = self.inner.lock().unwrap();

        let mut txn_builder = guard.build_tx();

        txn_builder
            .add_recipient(address.script_pubkey(), amount)
            .fee_rate(FeeRate::from_sat_per_vb(sat_vbyte).unwrap());

        let mut psbt = txn_builder.finish().unwrap();

        guard.sign(&mut psbt, SignOptions::default())?;

        let tx = psbt.extract_tx();

        match self.blockchain.blocking_client.broadcast(&tx) {
            Ok(_) => ..,
            Err(e) => return Err(anyhow!("Could not broadcast txn {}", e)),
        };

        Ok(tx.txid())
    }
}

impl FeeEstimator for DlcDevKitWallet {
    fn get_est_sat_per_1000_weight(&self, confirmation_target: ConfirmationTarget) -> u32 {
        self.fees
            .get(&confirmation_target)
            .unwrap()
            .load(Ordering::Acquire)
    }
}

impl dlc_manager::ContractSignerProvider for DlcDevKitWallet {
    type Signer = SimpleSigner;

    // Using the data deterministically generate a key id. From a child key.
    fn derive_signer_key_id(&self, _is_offer_party: bool, temp_id: [u8; 32]) -> [u8; 32] {
        let newest_index = self
            .inner
            .lock()
            .unwrap()
            .next_derivation_index(KeychainKind::External);
        let derivation_path = format!("m/86'/0'/0'/0'/{}", newest_index);
        let child_path = DerivationPath::from_str(&derivation_path)
            .expect("Not a valid derivation path to derive signer key.");
        let child_key = self
            .xprv
            .derive_priv(&self.secp, &child_path)
            .expect("Could not get child key for derivation path.");
        let mut hasher = Hasher::new();
        hasher.update(&temp_id);
        hasher.update(&child_key.encode());
        let hash = hasher.finalize();

        let mut key_id = [0u8; 32];
        key_id.copy_from_slice(hash.as_bytes());
        let public_key = PublicKey::from_secret_key(&self.secp, &child_key.private_key);
        self.simple_derive_signer.store_derived_key_id(
            newest_index,
            key_id,
            child_key.private_key,
            public_key,
        );

        key_id
    }

    fn derive_contract_signer(&self, key_id: [u8; 32]) -> Result<Self::Signer, ManagerError> {
        let index = self.simple_derive_signer.get_index_for_key_id(key_id);
        let derivation_path = format!("m/86'/0'/0'/0'/{}", index);
        let child_path = DerivationPath::from_str(&derivation_path)
            .expect("Not a valid derivation path to derive signer key.");
        let child_key = self
            .xprv
            .derive_priv(&self.secp, &child_path)
            .expect("Could not get child key for derivation path.");

        Ok(SimpleSigner::new(child_key.private_key))
    }

    fn get_secret_key_for_pubkey(
        &self,
        pubkey: &bitcoin::secp256k1::PublicKey,
    ) -> Result<bitcoin::secp256k1::SecretKey, ManagerError> {
        Ok(self.simple_derive_signer.get_secret_key(pubkey))
    }

    fn get_new_secret_key(&self) -> Result<bitcoin::secp256k1::SecretKey, ManagerError> {
        let newest_index = self
            .inner
            .lock()
            .unwrap()
            .next_derivation_index(KeychainKind::External);
        let derivation_path = format!("m/86'/0'/0'/0'/{}", newest_index);
        let child_path = DerivationPath::from_str(&derivation_path)
            .expect("Not a valid derivation path to derive signer key.");
        let child_key = self
            .xprv
            .derive_priv(&self.secp, &child_path)
            .expect("Could not get child key for derivation path.");
        Ok(child_key.private_key)
    }
}

impl dlc_manager::Wallet for DlcDevKitWallet {
    fn get_new_address(&self) -> Result<bitcoin::Address, ManagerError> {
        Ok(self
            .new_external_address()
            .unwrap()
            // .map_err(bdk_err_to_manager_err)?
            .address)
    }

    fn get_new_change_address(&self) -> Result<bitcoin::Address, ManagerError> {
        Ok(self
            .new_change_address()
            .unwrap()
            // .map_err(bdk_err_to_manager_err)?
            .address)
    }

    // TODO: Is this correct for the input?
    fn sign_psbt_input(
        &self,
        psbt: &mut bitcoin::psbt::PartiallySignedTransaction,
        _input_index: usize,
    ) -> Result<(), ManagerError> {
        self.inner
            .lock()
            .unwrap()
            .sign(psbt, bdk::SignOptions::default())
            .unwrap();
        // .map_err(bdk_err_to_manager_err)?;
        Ok(())
    }

    // TODO: Does BDK have reserved UTXOs?
    fn unreserve_utxos(&self, _outpoints: &[bitcoin::OutPoint]) -> Result<(), ManagerError> {
        Ok(())
    }

    fn import_address(&self, address: &bitcoin::Address) -> Result<(), ManagerError> {
        // might be ok, might not
        Ok(self.simple_derive_signer.import_address_to_storage(address))
    }

    // return all utxos
    // fixme use coin selector
    fn get_utxos_for_amount(
        &self,
        _amount: u64,
        _fee_rate: u64,
        _lock_utxos: bool,
    ) -> Result<Vec<dlc_manager::Utxo>, ManagerError> {
        let wallet = self.inner.lock().unwrap();

        let local_utxos = wallet.list_unspent();
        // .map_err(bdk_err_to_manager_err)?;

        let dlc_utxos = local_utxos
            .map(|utxo| {
                let address =
                    Address::from_script(&utxo.txout.script_pubkey, self.network).unwrap();
                dlc_manager::Utxo {
                    tx_out: utxo.txout.clone(),
                    outpoint: utxo.outpoint,
                    address,
                    redeem_script: ScriptBuf::new(),
                    reserved: false,
                }
            })
            .collect();

        Ok(dlc_utxos)
    }
}

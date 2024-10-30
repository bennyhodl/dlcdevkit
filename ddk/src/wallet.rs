use crate::error::{wallet_err_to_manager_err, WalletError};
use crate::{chain::EsploraClient, Storage};
use bdk_chain::{spk_client::FullScanRequest, Balance};
use bdk_esplora::EsploraExt;
use bdk_wallet::coin_selection::{BranchAndBoundCoinSelection, CoinSelectionAlgorithm};
use bdk_wallet::descriptor::IntoWalletDescriptor;
pub use bdk_wallet::LocalOutput;
use bdk_wallet::WalletPersister;
use bdk_wallet::{
    bitcoin::{
        bip32::Xpriv,
        secp256k1::{All, PublicKey, Secp256k1},
        Address, Network, Txid,
    },
    template::Bip84,
    AddressInfo, KeychainKind, PersistedWallet, SignOptions, Update, Wallet,
};
use bdk_wallet::{Utxo, WeightedUtxo};
use bitcoin::hashes::sha256::Hash as Sha256Hash;
use bitcoin::hashes::sha256::HashEngine;
use bitcoin::hashes::Hash;
use bitcoin::key::rand::{thread_rng, Fill};
use bitcoin::{secp256k1::SecretKey, Amount, FeeRate, ScriptBuf, Transaction};
use dlc_manager::{error::Error as ManagerError, SimpleSigner};
use lightning::chain::chaininterface::{ConfirmationTarget, FeeEstimator};
use std::collections::HashMap;
use std::io::Write;
use std::sync::atomic::AtomicU32;
use std::sync::RwLock;
use std::{
    collections::BTreeMap,
    sync::{atomic::Ordering, Arc},
};

/// Wrapper type to pass `crate::Storage` to a BDK wallet.
#[derive(Clone)]
pub struct WalletStorage(Arc<dyn Storage>);

impl WalletPersister for WalletStorage {
    type Error = WalletError;

    fn persist(persister: &mut Self, changeset: &bdk_wallet::ChangeSet) -> Result<(), Self::Error> {
        persister.0.as_ref().persist_bdk(changeset)
    }

    fn initialize(persister: &mut Self) -> Result<bdk_wallet::ChangeSet, Self::Error> {
        persister.0.as_ref().initialize_bdk()
    }
}

/// Internal [`bdk_wallet::PersistedWallet`] for ddk.
pub struct DlcDevKitWallet {
    /// BDK persisted wallet.
    pub wallet: Arc<RwLock<PersistedWallet<WalletStorage>>>,
    storage: WalletStorage,
    blockchain: Arc<EsploraClient>,
    network: Network,
    xprv: Xpriv,
    name: String,
    fees: Arc<HashMap<ConfirmationTarget, AtomicU32>>,
    secp: Secp256k1<All>,
}

const MIN_FEERATE: u32 = 253;

impl DlcDevKitWallet {
    pub fn new(
        name: &str,
        seed_bytes: &[u8; 32],
        esplora_url: &str,
        network: Network,
        storage: Arc<dyn Storage>,
    ) -> Result<DlcDevKitWallet, WalletError> {
        let secp = Secp256k1::new();

        let xprv = Xpriv::new_master(network, seed_bytes)?;

        let external_descriptor =
            Bip84(xprv, KeychainKind::External).into_wallet_descriptor(&secp, network)?;
        let internal_descriptor =
            Bip84(xprv, KeychainKind::Internal).into_wallet_descriptor(&secp, network)?;

        let mut storage = WalletStorage(storage);

        let load_wallet = Wallet::load()
            .descriptor(KeychainKind::External, Some(external_descriptor.clone()))
            .descriptor(KeychainKind::Internal, Some(internal_descriptor.clone()))
            .extract_keys()
            .check_network(network)
            .load_wallet(&mut storage)
            .map_err(|_| WalletError::WalletPersistanceError)?;

        let internal_wallet = match load_wallet {
            Some(w) => w,
            None => Wallet::create(external_descriptor, internal_descriptor)
                .network(network)
                .create_wallet(&mut storage)
                .map_err(|_| WalletError::WalletPersistanceError)?,
        };

        let wallet = Arc::new(RwLock::new(internal_wallet));

        let blockchain = Arc::new(
            EsploraClient::new(esplora_url, network)
                .map_err(|_| WalletError::WalletPersistanceError)?,
        );

        // TODO: Actually get fees. I don't think it's used for regular DLCs though
        let mut fees: HashMap<ConfirmationTarget, AtomicU32> = HashMap::new();
        fees.insert(ConfirmationTarget::UrgentOnChainSweep, AtomicU32::new(5000));
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
            wallet,
            storage,
            blockchain,
            network,
            xprv,
            fees,
            secp,
            name: name.to_string(),
        })
    }

    pub fn sync(&self) -> Result<(), WalletError> {
        let Ok(mut wallet) = self.wallet.try_write() else {
            tracing::error!("Could not get lock to sync wallet.");
            return Err(WalletError::Lock);
        };

        let mut storage = self.storage.clone();

        let prev_tip = wallet.latest_checkpoint();
        tracing::debug!(
            height = prev_tip.height(),
            "Syncing wallet with latest known height."
        );
        let sync_result = if prev_tip.height() == 0 {
            tracing::info!("Performing a full chain scan.");
            let spks = wallet
                .all_unbounded_spk_iters()
                .get(&KeychainKind::External)
                .unwrap()
                .to_owned();
            let chain = FullScanRequest::builder()
                .spks_for_keychain(KeychainKind::External, spks.clone())
                .chain_tip(prev_tip)
                .build();
            let sync = self.blockchain.blocking_client.full_scan(chain, 10, 1)?;
            Update {
                last_active_indices: sync.last_active_indices,
                tx_update: sync.tx_update,
                chain: sync.chain_update,
            }
        } else {
            let spks = wallet
                .start_sync_with_revealed_spks()
                .chain_tip(prev_tip)
                .build();
            let sync = self.blockchain.blocking_client.sync(spks, 1)?;
            let indices = wallet.derivation_index(KeychainKind::External).unwrap_or(0);
            let internal_index = wallet.derivation_index(KeychainKind::Internal).unwrap_or(0);
            let mut last_active_indices = BTreeMap::new();
            last_active_indices.insert(KeychainKind::External, indices);
            last_active_indices.insert(KeychainKind::Internal, internal_index);
            Update {
                last_active_indices,
                tx_update: sync.tx_update,
                chain: sync.chain_update,
            }
        };
        wallet.apply_update(sync_result)?;
        wallet
            .persist(&mut storage)
            .map_err(|_| WalletError::WalletPersistanceError)?;
        Ok(())
    }

    pub fn get_pubkey(&self) -> PublicKey {
        tracing::info!("Getting wallet public key.");
        PublicKey::from_secret_key(&self.secp, &self.xprv.private_key)
    }

    pub fn get_balance(&self) -> Result<Balance, WalletError> {
        let Ok(wallet) = self.wallet.try_read() else {
            tracing::error!("Could not get lock to sync wallet.");
            return Err(WalletError::Lock);
        };
        Ok(wallet.balance())
    }

    pub fn new_external_address(&self) -> Result<AddressInfo, WalletError> {
        let Ok(mut wallet) = self.wallet.try_write() else {
            tracing::error!("Could not get lock to sync wallet.");
            return Err(WalletError::Lock);
        };
        let mut storage = self.storage.clone();
        let address = wallet.next_unused_address(KeychainKind::External);
        let _ = wallet.persist(&mut storage);
        Ok(address)
    }

    pub fn new_change_address(&self) -> Result<AddressInfo, WalletError> {
        let Ok(mut wallet) = self.wallet.try_write() else {
            tracing::error!("Could not get lock to sync wallet.");
            return Err(WalletError::Lock);
        };
        let mut storage = self.storage.clone();
        let address = wallet.next_unused_address(KeychainKind::Internal);
        wallet
            .persist(&mut storage)
            .map_err(|_| WalletError::WalletPersistanceError)?;
        Ok(address)
    }

    pub fn send_to_address(
        &self,
        address: Address,
        amount: Amount,
        fee_rate: FeeRate,
    ) -> Result<Txid, WalletError> {
        let Ok(mut wallet) = self.wallet.try_write() else {
            tracing::error!("Could not get lock to sync wallet.");
            return Err(WalletError::Lock);
        };
        tracing::info!(
            address = address.to_string(),
            amount =? amount,
            "Sending transaction."
        );
        let mut txn_builder = wallet.build_tx();

        txn_builder
            .add_recipient(address.script_pubkey(), amount)
            .version(2)
            .fee_rate(fee_rate);

        let mut psbt = txn_builder.finish().unwrap();

        wallet.sign(&mut psbt, SignOptions::default())?;

        let tx = psbt.extract_tx()?;

        self.blockchain.blocking_client.broadcast(&tx)?;

        Ok(tx.compute_txid())
    }

    pub fn get_transactions(&self) -> Result<Vec<Arc<Transaction>>, WalletError> {
        let Ok(wallet) = self.wallet.try_read() else {
            tracing::error!("Could not get lock to sync wallet.");
            return Err(WalletError::Lock);
        };
        Ok(wallet
            .transactions()
            .into_iter()
            .map(|t| t.tx_node.tx)
            .collect::<Vec<Arc<Transaction>>>())
    }

    pub fn list_utxos(&self) -> Result<Vec<LocalOutput>, WalletError> {
        let Ok(wallet) = self.wallet.try_read() else {
            tracing::error!("Could not get lock to sync wallet.");
            return Err(WalletError::Lock);
        };
        Ok(wallet
            .list_unspent()
            .into_iter()
            .map(|utxo| utxo.to_owned())
            .collect())
    }

    fn next_derivation_index(&self) -> Result<u32, WalletError> {
        let Ok(wallet) = self.wallet.try_read() else {
            tracing::error!("Could not get lock to sync wallet.");
            return Err(WalletError::Lock);
        };

        Ok(wallet.next_derivation_index(KeychainKind::External))
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
        let mut random_bytes = [0u8; 32];
        let _ = random_bytes.try_fill(&mut thread_rng()).map_err(|e| {
            tracing::error!(
                "Did not create random bytes while generating key id. {:?}",
                e
            );
        });
        let mut hasher = HashEngine::default();
        hasher.write_all(&temp_id).unwrap();
        hasher.write_all(&random_bytes).unwrap();
        let hash: Sha256Hash = Hash::from_engine(hasher);

        // Might want to store this for safe backups.
        hash.to_byte_array()
    }

    fn derive_contract_signer(&self, key_id: [u8; 32]) -> Result<Self::Signer, ManagerError> {
        let child_key = SecretKey::from_slice(&key_id).expect("correct size");
        tracing::info!(
            key_id = hex::encode(key_id),
            "Derived secret key for contract."
        );
        Ok(SimpleSigner::new(child_key))
    }

    fn get_secret_key_for_pubkey(&self, _pubkey: &PublicKey) -> Result<SecretKey, ManagerError> {
        unreachable!("get_secret_key_for_pubkey is only used in channels.")
    }

    fn get_new_secret_key(&self) -> Result<SecretKey, ManagerError> {
        unreachable!("get_new_secret_key is only used for channels")
    }
}

impl dlc_manager::Wallet for DlcDevKitWallet {
    fn get_new_address(&self) -> Result<bitcoin::Address, ManagerError> {
        let address = self
            .new_external_address()
            .map_err(wallet_err_to_manager_err)?;
        tracing::info!(
            address = address.address.to_string(),
            "Revealed new address for contract."
        );
        Ok(address.address)
    }

    fn get_new_change_address(&self) -> Result<bitcoin::Address, ManagerError> {
        let address = self
            .new_change_address()
            .map_err(wallet_err_to_manager_err)?;
        tracing::info!(
            address = address.address.to_string(),
            "Revealed new change address for contract."
        );
        Ok(address.address)
    }

    fn sign_psbt_input(
        &self,
        psbt: &mut bitcoin::psbt::Psbt,
        input_index: usize,
    ) -> Result<(), ManagerError> {
        tracing::info!(
            input_index,
            inputs = psbt.inputs.len(),
            outputs = psbt.outputs.len(),
            "Signing psbt input for dlc manager."
        );
        let Ok(wallet) = self.wallet.try_read() else {
            tracing::error!("Could not get lock to sync wallet.");
            return Err(ManagerError::WalletError(WalletError::Lock.into()));
        };
        let sign_opts = SignOptions {
            trust_witness_utxo: true,
            ..Default::default()
        };

        let mut signed_psbt = psbt.clone();
        if let Err(e) = wallet.sign(&mut signed_psbt, sign_opts) {
            tracing::error!("Could not sign PSBT: {:?}", e);
            return Err(ManagerError::WalletError(WalletError::Signing(e).into()));
        };

        psbt.inputs[input_index] = signed_psbt.inputs[input_index].clone();

        Ok(())
    }

    // BDK does not have reserving UTXOs nor need it.
    fn unreserve_utxos(&self, _outpoints: &[bitcoin::OutPoint]) -> Result<(), ManagerError> {
        Ok(())
    }

    fn import_address(&self, address: &bitcoin::Address) -> Result<(), ManagerError> {
        tracing::warn!(address = address.to_string(), "Importing address.");
        // might be ok, might not
        Ok(self.storage.0.import_address_to_storage(address).unwrap())
    }

    // return all utxos
    fn get_utxos_for_amount(
        &self,
        amount: u64,
        fee_rate: u64,
        _lock_utxos: bool,
    ) -> Result<Vec<dlc_manager::Utxo>, ManagerError> {
        let local_utxos = self.list_utxos().map_err(wallet_err_to_manager_err)?;

        let utxos = local_utxos
            .iter()
            .map(|utxo| WeightedUtxo {
                satisfaction_weight: utxo.txout.weight(),
                utxo: Utxo::Local(utxo.clone()),
            })
            .collect::<Vec<WeightedUtxo>>();

        let selected_utxos = BranchAndBoundCoinSelection::new(
            Amount::MAX_MONEY.to_sat(),
            bdk_wallet::coin_selection::SingleRandomDraw::default(),
        )
        .coin_select(
            vec![],
            utxos,
            FeeRate::from_sat_per_vb(fee_rate).unwrap(),
            amount,
            ScriptBuf::new().as_script(),
            &mut thread_rng(),
        )
        .unwrap();

        let dlc_utxos = selected_utxos
            .selected
            .iter()
            .map(|utxo| {
                let address =
                    Address::from_script(&utxo.txout().script_pubkey, self.network).unwrap();
                dlc_manager::Utxo {
                    tx_out: utxo.txout().clone(),
                    outpoint: utxo.outpoint(),
                    address,
                    redeem_script: ScriptBuf::new(),
                    reserved: false,
                }
            })
            .collect();

        Ok(dlc_utxos)
    }
}

#[cfg(test)]
mod tests {
    use crate::test_util::TestSuite;
    use bitcoin::{key::rand::Fill, AddressType};
    use dlc_manager::ContractSignerProvider;

    #[test]
    fn address_is_p2wpkh() {
        let test = TestSuite::create_wallet("p2wpkh-address");
        let address = test.0.new_external_address().unwrap();
        assert_eq!(address.address.address_type().unwrap(), AddressType::P2wpkh)
    }

    #[test]
    fn derive_contract_signer() {
        let test = TestSuite::create_wallet("derive_contract_signer");
        let mut temp_key_id = [0u8; 32];
        temp_key_id
            .try_fill(&mut bitcoin::key::rand::thread_rng())
            .unwrap();
        let gen_key_id = test.0.derive_signer_key_id(true, temp_key_id);
        let key_info = test.0.derive_contract_signer(gen_key_id);
        assert!(key_info.is_ok())
    }
}

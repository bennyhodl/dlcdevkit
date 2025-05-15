use crate::error::{wallet_err_to_manager_err, WalletError};
use crate::{chain::EsploraClient, Storage};
use bdk_chain::{spk_client::FullScanRequest, Balance};
use bdk_esplora::EsploraAsyncExt;
use bdk_wallet::coin_selection::{
    BranchAndBoundCoinSelection, CoinSelectionAlgorithm, SingleRandomDraw,
};
use bdk_wallet::descriptor::IntoWalletDescriptor;
use bdk_wallet::AsyncWalletPersister;
pub use bdk_wallet::LocalOutput;
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
use ddk_manager::{error::Error as ManagerError, SimpleSigner};
use lightning::chain::chaininterface::{ConfirmationTarget, FeeEstimator};
use std::collections::HashMap;
use std::future::Future;
use std::io::Write;
use std::pin::Pin;
use std::sync::atomic::AtomicU32;
// use std::sync::RwLock;
use std::{
    collections::BTreeMap,
    sync::{atomic::Ordering, Arc},
};
use tokio::sync::Mutex;

type FutureResult<'a, T, E> = Pin<Box<dyn Future<Output = std::result::Result<T, E>> + Send + 'a>>;
type Result<T> = std::result::Result<T, WalletError>;

/// Wrapper type to pass `crate::Storage` to a BDK wallet.
#[derive(Clone)]
pub struct WalletStorage(Arc<dyn Storage>);

impl AsyncWalletPersister for WalletStorage {
    type Error = WalletError;

    fn initialize<'a>(
        persister: &'a mut Self,
    ) -> FutureResult<'a, bdk_wallet::ChangeSet, Self::Error>
    where
        Self: 'a,
    {
        Box::pin(persister.0.initialize_bdk())
    }

    fn persist<'a>(
        persister: &'a mut Self,
        changeset: &'a bdk_wallet::ChangeSet,
    ) -> FutureResult<'a, (), Self::Error>
    where
        Self: 'a,
    {
        tracing::info!("persist store");
        Box::pin(persister.0.persist_bdk(changeset))
    }
}

/// Internal [`bdk_wallet::PersistedWallet`] for ddk.
pub struct DlcDevKitWallet {
    /// BDK persisted wallet.
    pub wallet: Arc<Mutex<PersistedWallet<WalletStorage>>>,
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
    pub async fn new(
        name: &str,
        seed_bytes: &[u8; 32],
        esplora_url: &str,
        network: Network,
        storage: Arc<dyn Storage>,
    ) -> Result<DlcDevKitWallet> {
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
            .load_wallet_async(&mut storage)
            .await
            .map_err(|e| WalletError::WalletPersistanceError(e.to_string()))?;

        let internal_wallet = match load_wallet {
            Some(w) => w,
            None => Wallet::create(external_descriptor, internal_descriptor)
                .network(network)
                .create_wallet_async(&mut storage)
                .await
                .map_err(|e| WalletError::WalletPersistanceError(e.to_string()))?,
        };

        let wallet = Arc::new(Mutex::new(internal_wallet));

        let blockchain = Arc::new(
            EsploraClient::new(esplora_url, network)
                .map_err(|e| WalletError::Esplora(e.to_string()))?,
        );

        // not used for regular DLCs. only for channels
        let fees = Arc::new(fee_estimator());

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

    pub async fn sync(&self) -> Result<()> {
        let mut wallet = match self.wallet.try_lock() {
            Ok(w) => w,
            Err(e) => {
                tracing::error!(error =? e, "Could not get lock to sync wallet.");
                return Err(WalletError::Lock);
            }
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
            let sync = self
                .blockchain
                .async_client
                .full_scan(chain, 10, 1)
                .await
                .map_err(|e| WalletError::Esplora(e.to_string()))?;
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
            let sync = self
                .blockchain
                .async_client
                .sync(spks, 1)
                .await
                .map_err(|e| WalletError::Esplora(e.to_string()))?;
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
            .persist_async(&mut storage)
            .await
            .map_err(|e| WalletError::WalletPersistanceError(e.to_string()))?;
        Ok(())
    }

    pub fn get_pubkey(&self) -> PublicKey {
        tracing::info!("Getting wallet public key.");
        PublicKey::from_secret_key(&self.secp, &self.xprv.private_key)
    }

    pub fn get_balance(&self) -> Result<Balance> {
        let Ok(wallet) = self.wallet.try_lock() else {
            tracing::error!("Could not get lock to sync wallet.");
            return Err(WalletError::Lock);
        };
        Ok(wallet.balance())
    }

    pub async fn new_external_address(&self) -> Result<AddressInfo> {
        let Ok(mut wallet) = self.wallet.try_lock() else {
            tracing::error!("Could not get lock to sync wallet.");
            return Err(WalletError::Lock);
        };
        let mut storage = self.storage.clone();
        let address = wallet.next_unused_address(KeychainKind::External);
        let _ = wallet.persist_async(&mut storage).await;
        Ok(address)
    }

    pub async fn new_change_address(&self) -> Result<AddressInfo> {
        let Ok(mut wallet) = self.wallet.try_lock() else {
            tracing::error!("Could not get lock to sync wallet.");
            return Err(WalletError::Lock);
        };
        let mut storage = self.storage.clone();
        let address = wallet.next_unused_address(KeychainKind::Internal);
        let _ = wallet.persist_async(&mut storage).await;
        Ok(address)
    }

    pub async fn send_to_address(
        &self,
        address: Address,
        amount: Amount,
        fee_rate: FeeRate,
    ) -> Result<Txid> {
        let Ok(mut wallet) = self.wallet.try_lock() else {
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

        let mut psbt = txn_builder.finish()?;

        wallet.sign(&mut psbt, SignOptions::default())?;

        let tx = psbt.extract_tx().map_err(|_| WalletError::ExtractTx)?;

        self.blockchain.async_client.broadcast(&tx).await?;

        Ok(tx.compute_txid())
    }

    pub async fn send_all(&self, address: Address, fee_rate: FeeRate) -> Result<Txid> {
        let Ok(mut wallet) = self.wallet.try_lock() else {
            tracing::error!("Could not get lock to sync wallet.");
            return Err(WalletError::Lock);
        };

        tracing::info!(
            address = address.to_string(),
            "Sending all UTXOs to address."
        );

        let mut tx_builder = wallet.build_tx();
        tx_builder.fee_rate(fee_rate);
        tx_builder.drain_wallet();
        tx_builder.drain_to(address.script_pubkey());
        let mut psbt = tx_builder.finish().unwrap();
        wallet.sign(&mut psbt, SignOptions::default()).unwrap();
        let tx = psbt.extract_tx().map_err(|_| WalletError::ExtractTx)?;
        self.blockchain.async_client.broadcast(&tx).await?;

        Ok(tx.compute_txid())
    }

    pub fn get_transactions(&self) -> Result<Vec<Arc<Transaction>>> {
        let Ok(wallet) = self.wallet.try_lock() else {
            tracing::error!("Could not get lock to sync wallet.");
            return Err(WalletError::Lock);
        };
        Ok(wallet
            .transactions()
            .map(|t| t.tx_node.tx)
            .collect::<Vec<Arc<Transaction>>>())
    }

    pub fn list_utxos(&self) -> Result<Vec<LocalOutput>> {
        let Ok(wallet) = self.wallet.try_lock() else {
            tracing::error!("Could not get lock to sync wallet.");
            return Err(WalletError::Lock);
        };
        Ok(wallet.list_unspent().map(|utxo| utxo.to_owned()).collect())
    }

    fn next_derivation_index(&self) -> Result<u32> {
        let Ok(wallet) = self.wallet.try_lock() else {
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

impl ddk_manager::ContractSignerProvider for DlcDevKitWallet {
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

    fn derive_contract_signer(
        &self,
        key_id: [u8; 32],
    ) -> std::result::Result<Self::Signer, ManagerError> {
        let child_key = SecretKey::from_slice(&key_id).expect("correct size");
        tracing::info!(
            key_id = hex::encode(key_id),
            "Derived secret key for contract."
        );
        Ok(SimpleSigner::new(child_key))
    }

    fn get_secret_key_for_pubkey(
        &self,
        _pubkey: &PublicKey,
    ) -> std::result::Result<SecretKey, ManagerError> {
        unreachable!("get_secret_key_for_pubkey is only used in channels.")
    }

    fn get_new_secret_key(&self) -> std::result::Result<SecretKey, ManagerError> {
        unreachable!("get_new_secret_key is only used for channels")
    }
}

#[async_trait::async_trait]
impl ddk_manager::Wallet for DlcDevKitWallet {
    async fn get_new_address(&self) -> std::result::Result<bitcoin::Address, ManagerError> {
        let address = self
            .new_external_address()
            .await
            .map_err(wallet_err_to_manager_err)?;
        tracing::info!(
            address = address.address.to_string(),
            "Revealed new address for contract."
        );
        Ok(address.address)
    }

    async fn get_new_change_address(&self) -> std::result::Result<bitcoin::Address, ManagerError> {
        let address = self
            .new_change_address()
            .await
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
    ) -> std::result::Result<(), ManagerError> {
        tracing::info!(
            input_index,
            inputs = psbt.inputs.len(),
            outputs = psbt.outputs.len(),
            "Signing psbt input for dlc manager."
        );
        let Ok(wallet) = self.wallet.try_lock() else {
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
    fn unreserve_utxos(
        &self,
        _outpoints: &[bitcoin::OutPoint],
    ) -> std::result::Result<(), ManagerError> {
        Ok(())
    }

    fn import_address(&self, _address: &bitcoin::Address) -> std::result::Result<(), ManagerError> {
        Ok(())
    }

    // return all utxos
    fn get_utxos_for_amount(
        &self,
        amount: u64,
        fee_rate: u64,
        _lock_utxos: bool,
    ) -> std::result::Result<Vec<ddk_manager::Utxo>, ManagerError> {
        let local_utxos = self.list_utxos().map_err(wallet_err_to_manager_err)?;

        let utxos = local_utxos
            .iter()
            .map(|utxo| WeightedUtxo {
                satisfaction_weight: utxo.txout.weight(),
                utxo: Utxo::Local(utxo.clone()),
            })
            .collect::<Vec<WeightedUtxo>>();

        let selected_utxos =
            BranchAndBoundCoinSelection::new(Amount::MAX_MONEY.to_sat(), SingleRandomDraw)
                .coin_select(
                    vec![],
                    utxos,
                    FeeRate::from_sat_per_vb(fee_rate).unwrap(),
                    Amount::from_sat(amount),
                    ScriptBuf::new().as_script(),
                    &mut thread_rng(),
                )
                .map_err(|e| ManagerError::WalletError(Box::new(e)))?;

        let dlc_utxos = selected_utxos
            .selected
            .iter()
            .map(|utxo| {
                let address =
                    Address::from_script(&utxo.txout().script_pubkey, self.network).unwrap();
                ddk_manager::Utxo {
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

fn fee_estimator() -> HashMap<ConfirmationTarget, AtomicU32> {
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
    fees
}

#[cfg(test)]
mod tests {
    use std::{str::FromStr, sync::Arc, time::Duration};

    use crate::storage::memory::MemoryStorage;
    use bitcoin::{
        address::NetworkChecked, bip32::Xpriv, key::rand::Fill, Address, AddressType, Amount,
        FeeRate, Network,
    };
    use bitcoincore_rpc::RpcApi;
    use ddk_manager::{Blockchain, ContractSignerProvider};

    use super::DlcDevKitWallet;

    async fn create_wallet() -> DlcDevKitWallet {
        let esplora = std::env::var("ESPLORA_HOST").unwrap_or("http://localhost:30000".to_string());
        let storage = Arc::new(MemoryStorage::new());
        let mut entropy = [0u8; 64];
        entropy
            .try_fill(&mut bitcoin::key::rand::thread_rng())
            .unwrap();
        let xpriv = Xpriv::new_master(Network::Regtest, &entropy).unwrap();
        DlcDevKitWallet::new(
            "test".into(),
            &xpriv.private_key.secret_bytes(),
            &esplora,
            Network::Regtest,
            storage.clone(),
        )
        .await
        .unwrap()
    }

    fn generate_blocks(num: u64) {
        tracing::warn!("Generating {} blocks.", num);
        let bitcoind =
            std::env::var("BITCOIND_HOST").unwrap_or("http://localhost:18443".to_string());
        let auth = bitcoincore_rpc::Auth::UserPass("ddk".to_string(), "ddk".to_string());
        let client = bitcoincore_rpc::Client::new(&bitcoind, auth).unwrap();
        let previous_height = client.get_block_count().unwrap();

        let address = client.get_new_address(None, None).unwrap().assume_checked();
        client.generate_to_address(num, &address).unwrap();
        let mut cur_block_height = previous_height;
        while cur_block_height < previous_height + num {
            std::thread::sleep(Duration::from_secs(5));
            cur_block_height = client.get_block_count().unwrap();
        }
    }

    fn fund_address(address: &Address<NetworkChecked>) {
        let bitcoind =
            std::env::var("BITCOIND_HOST").unwrap_or("http://localhost:18443".to_string());
        let auth = bitcoincore_rpc::Auth::UserPass("ddk".to_string(), "ddk".to_string());
        let client = bitcoincore_rpc::Client::new(&bitcoind, auth).unwrap();
        client
            .send_to_address(
                address,
                Amount::from_btc(1.0).unwrap(),
                None,
                None,
                None,
                None,
                None,
                None,
            )
            .unwrap();
        generate_blocks(5)
    }

    #[tokio::test]
    async fn address_is_p2wpkh() {
        let test = create_wallet().await;
        let address = test.new_external_address().await.unwrap();
        assert_eq!(address.address.address_type().unwrap(), AddressType::P2wpkh)
    }

    #[tokio::test]
    async fn derive_contract_signer() {
        let test = create_wallet().await;
        let mut temp_key_id = [0u8; 32];
        temp_key_id
            .try_fill(&mut bitcoin::key::rand::thread_rng())
            .unwrap();
        let gen_key_id = test.derive_signer_key_id(true, temp_key_id);
        let key_info = test.derive_contract_signer(gen_key_id);
        assert!(key_info.is_ok())
    }

    #[tokio::test]
    async fn send_all() {
        let wallet = create_wallet().await;
        let network = wallet.blockchain.get_network().unwrap();
        let address = match network {
            Network::Regtest => "bcrt1qt0yrvs7qx8guvpqsx8u9mypz6t4zr3pxthsjkm",
            Network::Signet => "bcrt1q7h9uzwvyw29vrpujp69l7kce7e5w98mpn8kwsp",
            _ => "bcrt1qt0yrvs7qx8guvpqsx8u9mypz6t4zr3pxthsjkm",
        };
        let addr_one = wallet.new_external_address().await.unwrap().address;
        let addr_two = wallet.new_external_address().await.unwrap().address;
        fund_address(&addr_one);
        fund_address(&addr_two);
        wallet.sync().await.unwrap();
        assert!(wallet.get_balance().unwrap().confirmed > Amount::ZERO);
        wallet
            .send_all(
                Address::from_str(address).unwrap().assume_checked(),
                FeeRate::from_sat_per_vb(1).unwrap(),
            )
            .await
            .unwrap();
        generate_blocks(5);
        wallet.sync().await.unwrap();
        assert!(wallet.get_balance().unwrap().confirmed == Amount::ZERO)
    }
}

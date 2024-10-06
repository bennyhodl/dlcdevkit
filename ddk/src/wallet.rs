use crate::error::WalletError;
use crate::{chain::EsploraClient, signer::SignerInformation, storage::SledStorage, DdkStorage};
use bdk_chain::{spk_client::FullScanRequest, Balance};
use bdk_esplora::EsploraExt;
use bdk_wallet::{
    bitcoin::{
        bip32::{DerivationPath, Xpriv},
        secp256k1::{All, PublicKey, Secp256k1},
        Address, Network, Txid,
    },
    template::Bip84,
    AddressInfo, KeychainKind, LocalOutput, PersistedWallet, SignOptions, Update, Wallet,
};
use bitcoin::hashes::sha256::Hash as Sha256Hash;
use bitcoin::{
    hashes::{sha256::HashEngine, Hash},
    psbt::Psbt,
    secp256k1::SecretKey,
    Amount, FeeRate, ScriptBuf, Transaction,
};
use crossbeam::channel::{unbounded, Receiver, Sender};
use dlc_manager::{error::Error as ManagerError, SimpleSigner};
use lightning::chain::chaininterface::{ConfirmationTarget, FeeEstimator};
use std::{
    collections::BTreeMap,
    io::Write,
    sync::{atomic::Ordering, Arc},
};
use std::{collections::HashMap, path::Path};
use std::{str::FromStr, sync::atomic::AtomicU32};

/// Internal [bdk::Wallet] for ddk.
/// Uses eplora blocking for the [ddk::DlcDevKit] being sync only
/// Currently supports the file-based [bdk_file_store::Store]
pub struct DlcDevKitWallet<S> {
    pub blockchain: Arc<EsploraClient>,
    pub sender: Sender<WalletOperation>,
    pub network: Network,
    pub xprv: Xpriv,
    pub name: String,
    pub fees: Arc<HashMap<ConfirmationTarget, AtomicU32>>,
    derive_signer: Arc<S>,
    secp: Secp256k1<All>,
}

/// Messages that can be sent to the internal wallet.
pub enum WalletOperation {
    // Sync the wallet scrippubkeys to chain.
    Sync(Sender<Result<(), WalletError>>),
    // Retrieve wallet balance.
    Balance(Sender<Balance>),
    // Get a new, unused address for external use.
    NewExternalAddress(Sender<AddressInfo>),
    // Get a new, unused change address.
    NewChangeAddress(Sender<AddressInfo>),
    // Send an amount to an address.
    SendToAddress(Address, Amount, FeeRate, Sender<Result<Txid, WalletError>>),
    // Get all Transactions in the wallet.
    // TODO: Deref from Arc
    GetTransactions(Sender<Vec<Arc<Transaction>>>),
    // Get all UTXO's owned by the wallet.
    ListUtxos(Sender<Vec<LocalOutput>>),
    // Sign an input.
    SignPsbtInput(Psbt, usize, Sender<Result<Psbt, WalletError>>),
    // Get the next unused derivation path.
    NextDerivationIndex(Sender<u32>),
}

const MIN_FEERATE: u32 = 253;

impl<S: DdkStorage> DlcDevKitWallet<S> {
    pub fn new<P>(
        name: &str,
        seed_bytes: &[u8; 32],
        esplora_url: &str,
        network: Network,
        wallet_storage_path: P,
        derive_signer: Arc<S>,
    ) -> anyhow::Result<DlcDevKitWallet<S>>
    where
        P: AsRef<Path>,
    {
        let secp = Secp256k1::new();
        let wallet_storage_path = wallet_storage_path.as_ref().join("wallet-db");

        let xprv = Xpriv::new_master(network, seed_bytes)?;

        let external_descriptor = Bip84(xprv, KeychainKind::External);
        let internal_descriptor = Bip84(xprv, KeychainKind::Internal);
        // let file_store = bdk_file_store::Store::<ChangeSet>::open_or_create_new(b"ddk-wallet", wallet_storage_path)?;
        let mut storage = SledStorage::new(wallet_storage_path.to_str().unwrap())?;

        let load_wallet = Wallet::load()
            .descriptor(KeychainKind::External, Some(external_descriptor.clone()))
            .descriptor(KeychainKind::Internal, Some(internal_descriptor.clone()))
            .extract_keys()
            .check_network(network)
            .load_wallet(&mut storage)?;

        let mut wallet = match load_wallet {
            Some(w) => w,
            None => Wallet::create(external_descriptor, internal_descriptor)
                .network(network)
                .create_wallet(&mut storage)?,
        };

        let blockchain = Arc::new(EsploraClient::new(esplora_url, network)?);

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

        let (sender, receiver) = unbounded::<WalletOperation>();

        let esplora = blockchain.clone();
        std::thread::spawn(move || Self::run(&mut wallet, receiver, &mut storage, esplora));

        Ok(DlcDevKitWallet {
            blockchain,
            sender,
            network,
            xprv,
            fees,
            derive_signer,
            secp,
            name: name.to_string(),
        })
    }

    pub fn run(
        wallet: &mut PersistedWallet<SledStorage>,
        receiver: Receiver<WalletOperation>,
        db: &mut SledStorage,
        blockchain: Arc<EsploraClient>,
    ) {
        while let Ok(op) = receiver.recv() {
            match op {
                WalletOperation::Sync(responder) => {
                    let mut sync_inner =
                        |wallet: &mut PersistedWallet<SledStorage>| -> Result<(), WalletError> {
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
                                let sync = blockchain.blocking_client.full_scan(chain, 10, 1)?;
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
                                let sync = blockchain.blocking_client.sync(spks, 1)?;
                                let indices =
                                    wallet.derivation_index(KeychainKind::External).unwrap_or(0);
                                let internal_index =
                                    wallet.derivation_index(KeychainKind::Internal).unwrap_or(0);
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
                            wallet.persist(db)?;
                            Ok(())
                        };
                    let result = sync_inner(wallet);
                    if let Err(e) = responder.send(result) {
                        tracing::error!(message=?e, "Could not send message in sync message")
                    }
                }
                WalletOperation::Balance(responder) => {
                    let balance = wallet.balance();
                    if let Err(e) = responder.send(balance) {
                        tracing::error!(message=?e, "Could not send message in balance message")
                    }
                }
                WalletOperation::NewExternalAddress(responder) => {
                    let address = wallet.next_unused_address(KeychainKind::External);
                    // TODO: handle error.
                    let _ = wallet.persist(db);
                    if let Err(e) = responder.send(address) {
                        tracing::error!(message=?e, "Could not send message in balance message")
                    }
                }
                WalletOperation::NewChangeAddress(responder) => {
                    let address = wallet.next_unused_address(KeychainKind::Internal);
                    // TODO: handle error.
                    let _ = wallet.persist(db);
                    if let Err(e) = responder.send(address) {
                        tracing::error!(message=?e, "Could not send message in balance message")
                    }
                }
                WalletOperation::SendToAddress(address, amount, fee_rate, responder) => {
                    let send =
                        |wallet: &mut PersistedWallet<SledStorage>| -> Result<Txid, WalletError> {
                            let mut txn_builder = wallet.build_tx();

                            txn_builder
                                .add_recipient(address.script_pubkey(), amount)
                                .version(2)
                                .fee_rate(fee_rate);

                            let mut psbt = txn_builder.finish().unwrap();

                            wallet.sign(&mut psbt, SignOptions::default())?;

                            let tx = psbt.extract_tx()?;

                            blockchain.blocking_client.broadcast(&tx)?;

                            Ok(tx.compute_txid())
                        };
                    let txid = send(wallet);
                    if let Err(e) = responder.send(txid) {
                        tracing::error!(message=?e, "Could not send message to broadcast transaction.")
                    }
                }
                WalletOperation::GetTransactions(responder) => {
                    let transactions: Vec<Arc<Transaction>> = wallet
                        .transactions()
                        .into_iter()
                        .map(|t| t.tx_node.tx)
                        .collect();
                    if let Err(e) = responder.send(transactions) {
                        tracing::error!(message=?e, "Could not send message to get transactions.")
                    }
                }
                WalletOperation::ListUtxos(responder) => {
                    let utxos: Vec<LocalOutput> = wallet
                        .list_unspent()
                        .into_iter()
                        .map(|utxo| utxo.to_owned())
                        .collect();
                    if let Err(e) = responder.send(utxos) {
                        tracing::error!(message=?e, "Could not send message to get utxos.")
                    }
                }
                WalletOperation::NextDerivationIndex(responder) => {
                    let next_index = wallet.next_derivation_index(KeychainKind::External);
                    if let Err(e) = responder.send(next_index) {
                        tracing::error!(message=?e, "Could not send message to get utxos.")
                    }
                }
                WalletOperation::SignPsbtInput(mut psbt, _input_index, responder) => {
                    let sign = |psbt: &mut Psbt,
                                wallet: &mut PersistedWallet<SledStorage>|
                     -> Result<Psbt, WalletError> {
                        let mut sign_options = SignOptions::default();
                        sign_options.trust_witness_utxo = true;
                        if let Err(e) = wallet.sign(psbt, sign_options) {
                            tracing::error!("Could not sign PSBT: {:?}", e);
                            return Err(WalletError::Signing(e));
                        };
                        Ok(psbt.clone())
                    };
                    let sign_txn = sign(&mut psbt, wallet);
                    if let Err(e) = responder.send(sign_txn) {
                        tracing::error!(message=?e, "Could not send message to get utxos.")
                    }
                }
            }
        }
    }

    pub fn sync(&self) -> Result<(), WalletError> {
        let (sender, receiver) = unbounded();
        self.sender
            .send(WalletOperation::Sync(sender))
            .map_err(|e| WalletError::SendMessage(e.to_string()))?;
        receiver.recv()?
    }

    pub fn get_pubkey(&self) -> PublicKey {
        tracing::info!("Getting wallet public key.");
        PublicKey::from_secret_key(&self.secp, &self.xprv.private_key)
    }

    pub fn get_balance(&self) -> Result<Balance, WalletError> {
        let (sender, receiver) = unbounded();
        self.sender
            .send(WalletOperation::Balance(sender))
            .map_err(|e| WalletError::SendMessage(e.to_string()))?;
        Ok(receiver.recv()?)
    }

    pub fn new_external_address(&self) -> Result<AddressInfo, WalletError> {
        let (sender, receiver) = unbounded();
        self.sender
            .send(WalletOperation::NewExternalAddress(sender))
            .map_err(|e| WalletError::SendMessage(e.to_string()))?;
        Ok(receiver.recv()?)
    }

    pub fn new_change_address(&self) -> Result<AddressInfo, WalletError> {
        let (sender, receiver) = unbounded();
        self.sender
            .send(WalletOperation::NewChangeAddress(sender))
            .map_err(|e| WalletError::SendMessage(e.to_string()))?;
        Ok(receiver.recv()?)
    }

    pub fn send_to_address(
        &self,
        address: Address,
        amount: Amount,
        fee_rate: FeeRate,
    ) -> Result<Txid, WalletError> {
        tracing::info!(
            address = address.to_string(),
            amount =? amount,
            "Sending transaction."
        );
        let (sender, receiver) = unbounded();
        self.sender
            .send(WalletOperation::SendToAddress(
                address, amount, fee_rate, sender,
            ))
            .map_err(|e| WalletError::SendMessage(e.to_string()))?;
        receiver.recv()?
    }

    pub fn get_transactions(&self) -> Result<Vec<Arc<Transaction>>, WalletError> {
        let (sender, receiver) = unbounded();
        self.sender
            .send(WalletOperation::GetTransactions(sender))
            .map_err(|e| WalletError::SendMessage(e.to_string()))?;
        Ok(receiver.recv()?)
    }

    pub fn list_utxos(&self) -> Result<Vec<LocalOutput>, WalletError> {
        let (sender, receiver) = unbounded();
        self.sender
            .send(WalletOperation::ListUtxos(sender))
            .map_err(|e| WalletError::SendMessage(e.to_string()))?;
        Ok(receiver.recv()?)
    }
}

impl<S: DdkStorage> FeeEstimator for DlcDevKitWallet<S> {
    fn get_est_sat_per_1000_weight(&self, confirmation_target: ConfirmationTarget) -> u32 {
        self.fees
            .get(&confirmation_target)
            .unwrap()
            .load(Ordering::Acquire)
    }
}

impl<S: DdkStorage> dlc_manager::ContractSignerProvider for DlcDevKitWallet<S> {
    type Signer = SimpleSigner;

    // Using the data deterministically generate a key id. From a child key.
    fn derive_signer_key_id(&self, _is_offer_party: bool, temp_id: [u8; 32]) -> [u8; 32] {
        let (sender, receiver) = unbounded();
        self.sender
            .send(WalletOperation::NextDerivationIndex(sender))
            .expect("sender.");
        let newest_index = receiver.recv().expect("recv error");
        let derivation_path = format!("m/84'/0'/0'/0'/{}", newest_index);
        let child_path = DerivationPath::from_str(&derivation_path)
            .expect("Not a valid derivation path to derive signer key.");
        let child_key = self
            .xprv
            .derive_priv(&self.secp, &child_path)
            .expect("Could not get child key for derivation path.");

        let mut hasher = HashEngine::default();
        hasher.write_all(&temp_id).unwrap();
        hasher.write_all(&child_key.encode()).unwrap();
        let hash: Sha256Hash = Hash::from_engine(hasher);

        let mut key_id = [0u8; 32];
        key_id.copy_from_slice(hash.as_byte_array());
        let public_key = PublicKey::from_secret_key(&self.secp, &child_key.private_key);
        let signer_info = SignerInformation {
            index: newest_index,
            public_key,
            secret_key: child_key.private_key,
        };
        self.derive_signer
            .store_derived_key_id(key_id, signer_info)
            .unwrap();

        let key_id_string = hex::encode(&key_id);
        tracing::info!(key_id = key_id_string, "Derived new key id for signer.");
        key_id
    }

    fn derive_contract_signer(&self, key_id: [u8; 32]) -> Result<Self::Signer, ManagerError> {
        let info = self.derive_signer.get_key_information(key_id).unwrap();
        tracing::info!(
            key_id = hex::encode(key_id),
            "Derived secret key for contract."
        );
        Ok(SimpleSigner::new(info.secret_key))
    }

    fn get_secret_key_for_pubkey(&self, pubkey: &PublicKey) -> Result<SecretKey, ManagerError> {
        tracing::info!(
            pubkey = pubkey.to_string(),
            "Getting secret key from pubkey"
        );
        Ok(self.derive_signer.get_secret_key(pubkey).unwrap())
    }

    fn get_new_secret_key(&self) -> Result<SecretKey, ManagerError> {
        let (sender, receiver) = unbounded();
        self.sender
            .send(WalletOperation::NextDerivationIndex(sender))
            .expect("sender.");
        let newest_index = receiver.recv().expect("recv error");
        let derivation_path = format!("m/86'/0'/0'/0'/{}", newest_index);
        let child_path = DerivationPath::from_str(&derivation_path)
            .expect("Not a valid derivation path to derive signer key.");
        let child_key = self
            .xprv
            .derive_priv(&self.secp, &child_path)
            .expect("Could not get child key for derivation path.");
        tracing::info!("Retrieved new secret key.");
        Ok(child_key.private_key)
    }
}

impl<S: DdkStorage> dlc_manager::Wallet for DlcDevKitWallet<S> {
    fn get_new_address(&self) -> Result<bitcoin::Address, ManagerError> {
        tracing::info!("Retrieving new address for dlc manager");
        let (sender, receiver) = unbounded();
        self.sender
            .send(WalletOperation::NewExternalAddress(sender))
            .expect("couldn't send new address");
        Ok(receiver.recv().expect("no receive").address)
    }

    fn get_new_change_address(&self) -> Result<bitcoin::Address, ManagerError> {
        tracing::info!("Retrieving new change address for dlc manager");
        let (sender, receiver) = unbounded();
        self.sender
            .send(WalletOperation::NewChangeAddress(sender))
            .expect("couldn't send new address");
        Ok(receiver.recv().expect("no receive").address)
    }

    fn sign_psbt_input(
        &self,
        psbt: &mut bitcoin::psbt::Psbt,
        input_index: usize,
    ) -> Result<(), ManagerError> {
        tracing::info!("Signing psbt input for dlc manager.");
        let (sender, receiver) = unbounded();
        self.sender
            .send(WalletOperation::SignPsbtInput(
                psbt.clone(),
                input_index,
                sender,
            ))
            .expect("no send psbt input");
        let signed_psbt = receiver
            .recv()
            .expect("receiver no sign")
            .expect("psbt not signed");
        let tx = psbt.unsigned_tx.clone();
        let tx_input = tx.input[input_index].clone();
        psbt.inputs[input_index].final_script_sig = Some(tx_input.script_sig);
        psbt.inputs[input_index].final_script_witness = Some(tx_input.witness);
        *psbt = signed_psbt;
        Ok(())
    }

    // TODO: Does BDK have reserved UTXOs?
    fn unreserve_utxos(&self, _outpoints: &[bitcoin::OutPoint]) -> Result<(), ManagerError> {
        Ok(())
    }

    fn import_address(&self, address: &bitcoin::Address) -> Result<(), ManagerError> {
        // might be ok, might not
        Ok(self
            .derive_signer
            .import_address_to_storage(address)
            .unwrap())
    }

    // return all utxos
    // fixme use coin selector
    fn get_utxos_for_amount(
        &self,
        _amount: u64,
        _fee_rate: u64,
        _lock_utxos: bool,
    ) -> Result<Vec<dlc_manager::Utxo>, ManagerError> {
        let (sender, receiver) = unbounded();
        self.sender
            .send(WalletOperation::ListUtxos(sender))
            .expect("list utxos");
        let local_utxos = receiver.recv().expect("no receiver");

        let dlc_utxos = local_utxos
            .iter()
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

#[cfg(test)]
mod tests {
    use crate::test_util::TestSuite;
    use bitcoin::{key::rand::Fill, AddressType};
    use dlc_manager::ContractSignerProvider;

    #[test]
    fn address_is_p2wpkh() {
        let test = TestSuite::create_wallet("p2wpkh-address");
        let address = test.new_external_address().unwrap();
        assert_eq!(address.address.address_type().unwrap(), AddressType::P2wpkh)
    }

    #[test]
    fn derive_contract_signer() {
        let test = TestSuite::create_wallet("derive_contract_signer");
        let mut temp_key_id = [0u8; 32];
        temp_key_id
            .try_fill(&mut bitcoin::key::rand::thread_rng())
            .unwrap();
        let gen_key_id = test.derive_signer_key_id(true, temp_key_id);
        let key_info = test.derive_contract_signer(gen_key_id);
        assert!(key_info.is_ok())
    }
}

#![allow(dead_code)]
mod ernest;
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
        Address, Txid, Script,
        util::bip32::{ExtendedPubKey, ExtendedPrivKey, ChildNumber}
    },
    blockchain::{esplora::EsploraError, EsploraBlockchain},
    database::SqliteDatabase,
    template::Bip84,
    wallet::{AddressIndex, AddressInfo},
    Balance, FeeRate, KeychainKind, SignOptions, SyncOptions, Wallet
};
use io::{create_ernest_dir_with_wallet, get_ernest_dir};
use lightning::chain::chaininterface::{ConfirmationTarget, FeeEstimator};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use std::{collections::HashMap, sync::Mutex};
use dlc_manager::error::Error as ManagerError;

pub struct ErnestWallet {
    pub blockchain: EsploraBlockchain,
    pub inner: Mutex<Wallet<SqliteDatabase>>,
    pub network: Network,
    pub xprv: ExtendedPrivKey,
    pub name: String,
    fees: Arc<HashMap<ConfirmationTarget, AtomicU32>>,
}

#[derive(Debug)]
enum ErnestWalletError {
    Bdk(bdk::Error),
    Esplora(bdk::blockchain::esplora::EsploraError)
}

impl From<bdk::Error> for ErnestWalletError {
    fn from(value: bdk::Error) -> ErnestWalletError {
        ErnestWalletError::Bdk(value)
    }
}

impl From<bdk::blockchain::esplora::EsploraError> for ErnestWalletError {
    fn from(value: bdk::blockchain::esplora::EsploraError) -> Self {
        ErnestWalletError::Esplora(value)
    }
}

impl From<ErnestWalletError> for ManagerError {
    fn from(e: ErnestWalletError) -> ManagerError {
        match e {
            ErnestWalletError::Bdk(e) => ManagerError::WalletError(Box::new(e)),
            ErnestWalletError::Esplora(_) => ManagerError::BlockchainError
        }
    }
}

fn bdk_err_to_manager_err(e: bdk::Error) -> ManagerError {
    ErnestWalletError::Bdk(e).into()
}

fn esplora_err_to_manager_err(e: EsploraError) -> ManagerError {
    ErnestWalletError::Esplora(e).into()
}

const MIN_FEERATE: u32 = 253;

impl ErnestWallet {
    pub fn new(
        name: String,
        esplora_url: String,
        network: Network,
    ) -> anyhow::Result<ErnestWallet> {
        let wallet_dir = create_ernest_dir_with_wallet(name.clone())?;

        // Save the seed to the OS keychain. Not in home directory.
        let ernest_dir = wallet_dir
            .clone()
            .parent()
            .unwrap()
            .join(format!("{}_seed", name));

        let xprv = io::read_or_generate_xprv(ernest_dir.clone(), network)?;

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

impl FeeEstimator for ErnestWallet {
    fn get_est_sat_per_1000_weight(&self, confirmation_target: ConfirmationTarget) -> u32 {
        self.fees
            .get(&confirmation_target)
            .unwrap()
            .load(Ordering::Acquire)
    }
}

impl dlc_manager::Wallet for ErnestWallet {
    fn get_new_address(&self) -> Result<Address, ManagerError> {
        Ok(self.new_external_address().map_err(bdk_err_to_manager_err)?.address)
    }

    fn get_new_secret_key(
        &self,
    ) -> Result<bitcoin::secp256k1::SecretKey, ManagerError> {
        let network_index = if self.network == Network::Bitcoin {
            ChildNumber::from_hardened_idx(0).unwrap()
        } else {
            ChildNumber::from_hardened_idx(1).unwrap()
        };
        
        let path = [
            ChildNumber::from_hardened_idx(420).unwrap(),
            network_index,
            ChildNumber::from_hardened_idx(0).unwrap()
        ];

        let secp = Secp256k1::new();

        Ok(self.xprv.derive_priv(&secp, &path).unwrap().private_key)
    }

    fn import_address(&self, _address: &Address) -> Result<(), ManagerError> {
        // might be ok, might not
        Ok(())
    }

    // return all utxos
    // fixme
    fn get_utxos_for_amount(
        &self,
        _amount: u64,
        _fee_rate: Option<u64>,
        _lock_utxos: bool,
    ) -> Result<Vec<dlc_manager::Utxo>, ManagerError> {
        let wallet = self.inner.lock().unwrap();

        let local_utxos = wallet.list_unspent().map_err(bdk_err_to_manager_err)?;

        let dlc_utxos = local_utxos.iter().map(|utxo| {
            let address = Address::from_script(&utxo.txout.script_pubkey, self.network).unwrap();
            dlc_manager::Utxo {
                tx_out: utxo.txout.clone(),
                outpoint: utxo.outpoint,
                address,
                redeem_script: Script::new(),
                reserved: false
            }
        }).collect();

        Ok(dlc_utxos)

    }
}

impl dlc_manager::Signer for ErnestWallet {
    // Waiting for rust-dlc PR
    fn sign_tx_input(
        &self,
        _tx: &mut bitcoin::Transaction,
        _input_index: usize,
        _tx_out: &bitcoin::TxOut,
        _redeem_script: Option<bitcoin::Script>,
    ) -> Result<(), ManagerError> {
        unimplemented!("Waiting for rust-dlc PR for sign psbt")
    }

    fn get_secret_key_for_pubkey(
        &self,
        _pubkey: &PublicKey,
    ) -> Result<bitcoin::secp256k1::SecretKey, ManagerError> {
        let network_index = if self.network == Network::Bitcoin {
            ChildNumber::from_hardened_idx(0).unwrap()
        } else {
            ChildNumber::from_hardened_idx(1).unwrap()
        };
        
        let path = [
            ChildNumber::from_hardened_idx(420).unwrap(),
            network_index,
            ChildNumber::from_hardened_idx(0).unwrap()
        ];

        let secp = Secp256k1::new();

        Ok(self.xprv.derive_priv(&secp, &path).unwrap().private_key)
    }
}

impl dlc_manager::Blockchain for ErnestWallet {
    fn get_network(
        &self,
    ) -> Result<bitcoin::network::constants::Network, ManagerError> {
        Ok(self.network)
    }

    fn get_transaction(
        &self,
        tx_id: &Txid,
    ) -> Result<bitcoin::Transaction, ManagerError> {
        let wallet = self.inner.lock().unwrap();

        let txn = wallet.get_tx(tx_id, false).map_err(bdk_err_to_manager_err)?;

        match txn {
            Some(txn) => Ok(txn.transaction.unwrap()),
            None => Err(bdk_err_to_manager_err(bdk::Error::TransactionNotFound))
        }
    }

    fn send_transaction(
        &self,
        transaction: &bitcoin::Transaction,
    ) -> Result<(), ManagerError> {
        Ok(self.blockchain.broadcast(transaction).map_err(esplora_err_to_manager_err)?)
    }

    fn get_block_at_height(
        &self,
        height: u64,
    ) -> Result<bitcoin::Block, ManagerError> {
        let block_hash = self.blockchain.get_block_hash(height as u32).map_err(esplora_err_to_manager_err)?;
        
        let block = self.blockchain.get_block_by_hash(&block_hash).map_err(esplora_err_to_manager_err)?;

        match block {
            Some(block) => Ok(block),
            None => Err(esplora_err_to_manager_err(EsploraError::HttpResponse(404)))
        }
    }

    fn get_blockchain_height(&self) -> Result<u64, ManagerError> {
        // Ok(self.blockchain.get_height().map_err(esplora_err_to_manager_err)? as u64)
        unreachable!("Get block height should only used for channels.")
    }

    fn get_transaction_confirmations(
        &self,
        tx_id: &Txid,
    ) -> Result<u32, ManagerError> {
        let txn = self.blockchain.get_tx_status(tx_id).map_err(esplora_err_to_manager_err)?;
        let tip_height = self.blockchain.get_height().map_err(esplora_err_to_manager_err)?;

        match txn {
            Some(txn) => {
                if txn.confirmed {
                    match txn.block_height {
                        Some(height) => Ok(tip_height - height),
                        None => Ok(0)
                    }
                } else {
                    Err(esplora_err_to_manager_err(EsploraError::TransactionNotFound(*tx_id)))
                }
            },
            None => Err(esplora_err_to_manager_err(EsploraError::TransactionNotFound(*tx_id)))
        }
    }
}

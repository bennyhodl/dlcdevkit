use crate::{
    error::{bdk_err_to_manager_err, esplora_err_to_manager_err},
    wallet::ErnestWallet,
};
use bdk::{
    bitcoin::{
        bip32::ChildNumber,
        secp256k1::{PublicKey, Secp256k1},
        Address, Network, Script, Txid,
    },
    blockchain::esplora::EsploraError,
};
use bitcoin::ScriptBuf;
use dlc_manager::{error::Error as ManagerError, SimpleSigner};
use lightning::chain::chaininterface::{ConfirmationTarget, FeeEstimator};

impl FeeEstimator for ErnestWallet {
    // TODO: fix me
    fn get_est_sat_per_1000_weight(&self, _confirmation_target: ConfirmationTarget) -> u32 {
        // self.fees
        //     .get(&confirmation_target)
        //     .unwrap()
        //     .load(Ordering::Acquire)
        1
    }
}

impl dlc_manager::ContractSignerProvider for ErnestWallet {
    type Signer = SimpleSigner;

    fn derive_signer_key_id(&self, _is_offer_party: bool, temp_id: [u8; 32]) -> [u8; 32] {
        temp_id
    }

    fn derive_contract_signer(&self, _key_id: [u8; 32]) -> Result<Self::Signer, ManagerError> {
        Ok(SimpleSigner::new(self.xprv.private_key))
    }

    fn get_secret_key_for_pubkey(
        &self,
        _pubkey: &bitcoin::secp256k1::PublicKey,
    ) -> Result<bitcoin::secp256k1::SecretKey, ManagerError> {
        unimplemented!()
    }

    fn get_new_secret_key(&self) -> Result<bitcoin::secp256k1::SecretKey, ManagerError> {
        unimplemented!()
    }
}

impl dlc_manager::Wallet for ErnestWallet {
    fn get_new_address(&self) -> Result<bitcoin::Address, ManagerError> {
        Ok(self
            .new_external_address()
            .map_err(bdk_err_to_manager_err)?
            .address)
    }

    fn get_new_change_address(&self) -> Result<bitcoin::Address, ManagerError> {
        Ok(self
            .new_change_address()
            .map_err(bdk_err_to_manager_err)?
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
            .map_err(bdk_err_to_manager_err)?;
        Ok(())
    }

    // TODO: Does BDK have reserved UTXOs?
    fn unreserve_utxos(&self, _outpoints: &[bitcoin::OutPoint]) -> Result<(), ManagerError> {
        Ok(())
    }

    fn import_address(&self, _address: &bitcoin::Address) -> Result<(), ManagerError> {
        // might be ok, might not
        Ok(())
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

        let local_utxos = wallet.list_unspent().map_err(bdk_err_to_manager_err)?;

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

impl dlc_manager::Blockchain for ErnestWallet {
    fn get_network(&self) -> Result<bitcoin::network::constants::Network, ManagerError> {
        Ok(self.network)
    }

    fn get_transaction(&self, tx_id: &bitcoin::Txid) -> Result<bitcoin::Transaction, ManagerError> {
        let wallet = self.inner.lock().unwrap();

        let txn = wallet
            .get_tx(tx_id, false)
            .map_err(bdk_err_to_manager_err)?;

        match txn {
            Some(txn) => Ok(txn.transaction.unwrap()),
            None => Err(bdk_err_to_manager_err(bdk::Error::TransactionNotFound)),
        }
    }

    fn send_transaction(&self, transaction: &bitcoin::Transaction) -> Result<(), ManagerError> {
        Ok(self
            .blockchain
            .broadcast(transaction)
            .map_err(esplora_err_to_manager_err)?)
    }

    fn get_block_at_height(&self, height: u64) -> Result<bitcoin::Block, ManagerError> {
        let block_hash = self
            .blockchain
            .get_block_hash(height as u32)
            .map_err(esplora_err_to_manager_err)?;

        let block = self
            .blockchain
            .get_block_by_hash(&block_hash)
            .map_err(esplora_err_to_manager_err)?;

        match block {
            Some(block) => Ok(block),
            None => Err(esplora_err_to_manager_err(EsploraError::HttpResponse(404))),
        }
    }

    fn get_blockchain_height(&self) -> Result<u64, ManagerError> {
        Ok(self
            .blockchain
            .get_height()
            .map_err(esplora_err_to_manager_err)? as u64)
    }

    fn get_transaction_confirmations(&self, tx_id: &bitcoin::Txid) -> Result<u32, ManagerError> {
        let txn = self
            .blockchain
            .get_tx_status(tx_id)
            .map_err(esplora_err_to_manager_err)?;
        let tip_height = self
            .blockchain
            .get_height()
            .map_err(esplora_err_to_manager_err)?;

        if txn.confirmed {
            match txn.block_height {
                Some(height) => Ok(tip_height - height),
                None => Ok(0),
            }
        } else {
            Err(esplora_err_to_manager_err(
                EsploraError::TransactionNotFound(*tx_id),
            ))
        }
    }
}

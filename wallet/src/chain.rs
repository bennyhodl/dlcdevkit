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

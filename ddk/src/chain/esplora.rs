use std::sync::Arc;
use std::time::Duration;

use crate::error::{esplora_err_to_manager_err, Error};
use crate::logger::Logger;
use crate::logger::{log_error, log_info, log_warn, WriteLog};
use bdk_esplora::esplora_client::Error as EsploraError;
use bdk_esplora::esplora_client::{AsyncClient, BlockingClient, Builder};
use bitcoin::Network;
use bitcoin::{consensus::encode, Transaction, Txid};
use ddk_manager::error::Error as ManagerError;
use lightning::chain::chaininterface::{ConfirmationTarget, FeeEstimator};

/// Esplora client for getting chain information. Holds both a blocking
/// and an async client.
///
/// Used by rust-dlc for getting transactions related to DLC contracts.
/// Used by bdk to sync the wallet and track transaction.
#[derive(Debug)]
pub struct EsploraClient {
    pub blocking_client: BlockingClient,
    pub async_client: AsyncClient,
    network: Network,
    logger: Arc<Logger>,
}

impl EsploraClient {
    pub fn new(
        esplora_host: &str,
        network: Network,
        logger: Arc<Logger>,
    ) -> Result<EsploraClient, Error> {
        let builder = Builder::new(esplora_host).timeout(Duration::from_secs(5).as_secs());
        let blocking_client = builder.clone().build_blocking();
        let async_client = builder.build_async()?;
        Ok(EsploraClient {
            blocking_client,
            async_client,
            network,
            logger,
        })
    }
}

/// Implements the `ddk_manager::Blockchain` interface. Grabs chain related information
/// regarding DLC transactions.
#[async_trait::async_trait]
impl ddk_manager::Blockchain for EsploraClient {
    fn get_network(&self) -> Result<Network, ManagerError> {
        Ok(self.network)
    }

    async fn get_transaction(&self, tx_id: &Txid) -> Result<Transaction, ManagerError> {
        log_info!(
            self.logger,
            "Querying for transaction. txid={}",
            tx_id.to_string()
        );
        let txn = self
            .async_client
            .get_tx(tx_id)
            .await
            .map_err(esplora_err_to_manager_err)?;

        match txn {
            Some(txn) => Ok(txn),
            None => Err(esplora_err_to_manager_err(
                EsploraError::TransactionNotFound(*tx_id),
            )),
        }
    }

    async fn send_transaction(
        &self,
        transaction: &bitcoin::Transaction,
    ) -> Result<(), ManagerError> {
        let txid = transaction.compute_txid();
        let tx_hex = encode::serialize_hex(transaction);

        log_info!(
            self.logger,
            "Broadcasting transaction. txid={}, tx_hex={}, num_inputs={}, num_outputs={}",
            txid.to_string(),
            tx_hex,
            transaction.input.len(),
            transaction.output.len()
        );

        if let Ok(status) = self
            .async_client
            .get_tx_status(&transaction.compute_txid())
            .await
        {
            log_warn!(
                self.logger,
                "Transaction already submitted. txid={}",
                transaction.compute_txid().to_string()
            );
            if status.confirmed {
                return Ok(());
            }
        };

        if let Err(e) = self.async_client.broadcast(transaction).await {
            log_error!(
                self.logger,
                "Could not broadcast transaction. txid={}, tx_hex={}, error={}",
                txid.to_string(),
                tx_hex,
                e.to_string()
            );

            return Err(esplora_err_to_manager_err(e));
        }

        Ok(())
    }

    #[tracing::instrument(skip(self))]
    async fn get_block_at_height(&self, height: u64) -> Result<bitcoin::Block, ManagerError> {
        let block_hash = self
            .async_client
            .get_block_hash(height as u32)
            .await
            .map_err(esplora_err_to_manager_err)?;

        let block = self
            .async_client
            .get_block_by_hash(&block_hash)
            .await
            .map_err(esplora_err_to_manager_err)?;

        match block {
            Some(block) => Ok(block),
            None => Err(esplora_err_to_manager_err(EsploraError::HttpResponse {
                status: 404,
                message: "Block not found in esplore".into(),
            })),
        }
    }

    #[tracing::instrument(skip(self))]
    async fn get_blockchain_height(&self) -> Result<u64, ManagerError> {
        Ok(self
            .async_client
            .get_height()
            .await
            .map_err(esplora_err_to_manager_err)? as u64)
    }

    #[tracing::instrument(skip(self), fields(txid = tx_id.to_string()))]
    async fn get_transaction_confirmations(
        &self,
        tx_id: &bitcoin::Txid,
    ) -> Result<u32, ManagerError> {
        let txn = self
            .async_client
            .get_tx_status(tx_id)
            .await
            .map_err(esplora_err_to_manager_err)?;
        let tip_height = self
            .async_client
            .get_height()
            .await
            .map_err(esplora_err_to_manager_err)?;

        if txn.confirmed {
            match txn.block_height {
                Some(height) => Ok(tip_height - height),
                None => Ok(0),
            }
        } else {
            Ok(0)
        }
    }
}

impl FeeEstimator for EsploraClient {
    fn get_est_sat_per_1000_weight(&self, _confirmation_target: ConfirmationTarget) -> u32 {
        1
    }
}

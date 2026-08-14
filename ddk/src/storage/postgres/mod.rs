use super::sqlx::{ContractData, ContractMetadata, SqlxError};
use crate::error::{StorageError, WalletError};
use crate::logger::Logger;
use crate::logger::{log_info, WriteLog};
use crate::Storage;
use crate::{
    error::to_storage_error,
    util::ser::{deserialize_contract, serialize_contract, ContractPrefix},
};
use bdk_chain::{
    local_chain, tx_graph, Anchor, ConfirmationBlockTime, DescriptorExt, DescriptorId, Merge,
};
use bdk_wallet::bitcoin::{
    self,
    consensus::{self, Decodable},
    hashes::{sha256, Hash},
    Amount, BlockHash, Network, OutPoint, ScriptBuf, TxOut, Txid,
};
use bdk_wallet::chain as bdk_chain;
use bdk_wallet::descriptor::{Descriptor, ExtendedDescriptor};
use bdk_wallet::keys::DescriptorPublicKey;
use bdk_wallet::ChangeSet;
use bdk_wallet::KeychainKind;
use bdk_wallet::KeychainKind::{External, Internal};
use ddk_manager::{
    contract::{
        offered_contract::OfferedContract, ser::Serializable, signed_contract::SignedContract,
        Contract, PreClosedContract,
    },
    Storage as ManagerStorage,
};
use serde_json::json;
use sqlx::pool::PoolOptions;
use sqlx::postgres::PgRow;
use sqlx::{FromRow, Pool, Postgres, Row, Transaction};
use std::collections::BTreeMap;
use std::str::FromStr;
use std::sync::Arc;

/// Default maximum number of connections held in the Postgres pool.
///
/// Production deployments under load should raise this (20+) via the
/// `DATABASE_MAX_CONNECTIONS` environment variable to avoid connection
/// exhaustion.
pub const DEFAULT_MAX_CONNECTIONS: u32 = 5;

/// Resolves the maximum size of the Postgres connection pool.
///
/// Reads the `DATABASE_MAX_CONNECTIONS` environment variable, falling back to
/// [`DEFAULT_MAX_CONNECTIONS`] when the variable is unset, unparseable, or zero.
fn max_connections_from_env() -> u32 {
    std::env::var("DATABASE_MAX_CONNECTIONS")
        .ok()
        .and_then(|val| val.parse::<u32>().ok())
        .filter(|n| *n > 0)
        .unwrap_or(DEFAULT_MAX_CONNECTIONS)
}

/// Manages a pool of database connections.
#[derive(Debug)]
pub struct PostgresStore {
    pub(crate) pool: Pool<Postgres>,
    wallet_name: String,
    logger: Arc<Logger>,
}

impl PostgresStore {
    pub async fn new(
        url: &str,
        migrations: bool,
        logger: Arc<Logger>,
        wallet_name: String,
    ) -> Result<Self, StorageError> {
        let max_connections = max_connections_from_env();
        log_info!(
            logger,
            "Creating postgres pool. max_connections={}",
            max_connections
        );
        let pool = PoolOptions::<Postgres>::new()
            .max_connections(max_connections)
            .connect(url)
            .await
            .map_err(|e| StorageError::Sqlx(e.into()))?;
        // TODO: inline migrations
        if migrations {
            log_info!(logger, "Migrating postgres");
            sqlx::migrate!("src/storage/postgres/migrations")
                .run(&pool)
                .await
                .map_err(|e| StorageError::Sqlx(e.into()))?;
        }

        Ok(Self {
            pool,
            logger,
            wallet_name,
        })
    }

    pub async fn get_contract_metadata(
        &self,
        states: Option<Vec<ContractPrefix>>,
    ) -> Result<Vec<ContractMetadata>, StorageError> {
        let rows = if let Some(states) = states {
            let placeholders = (1..=states.len())
                .map(|i| format!("${i}"))
                .collect::<Vec<_>>()
                .join(", ");

            let query = format!("SELECT * FROM contract_metadata WHERE state IN ({placeholders})");

            let mut query = sqlx::query_as::<_, ContractMetadata>(&query);

            for state in states {
                query = query.bind(state as i16);
            }

            query
                .fetch_all(&self.pool)
                .await
                .map_err(|e| StorageError::Sqlx(e.into()))?
        } else {
            sqlx::query_as::<Postgres, ContractMetadata>("SELECT * FROM contract_metadata")
                .fetch_all(&self.pool)
                .await
                .map_err(|e| StorageError::Sqlx(e.into()))?
        };
        Ok(rows)
    }

    pub async fn get_contract_metadata_by_id(
        &self,
        id: &str,
    ) -> Result<ContractMetadata, StorageError> {
        let row = sqlx::query_as::<Postgres, ContractMetadata>(
            "SELECT * FROM contract_metadata WHERE id = $1",
        )
        .bind(id)
        .fetch_one(&self.pool)
        .await
        .map_err(|e| StorageError::Sqlx(e.into()))?;
        Ok(row)
    }

    pub async fn get_offer_metadata(&self) -> Result<Vec<ContractMetadata>, StorageError> {
        let rows = sqlx::query_as::<Postgres, ContractMetadata>(
            "SELECT * FROM contract_metadata WHERE state = 1",
        )
        .fetch_all(&self.pool)
        .await
        .map_err(|e| StorageError::Sqlx(e.into()))?;
        Ok(rows)
    }

    #[tracing::instrument(skip(self))]
    pub(crate) async fn read(&self) -> Result<ChangeSet, StorageError> {
        log_info!(
            self.logger,
            "Reading changeset from postgres. wallet_name={}",
            self.wallet_name
        );
        let mut tx = self
            .pool
            .begin()
            .await
            .map_err(|e| StorageError::Sqlx(e.into()))?;
        let mut changeset = ChangeSet::default();
        let sql =
            "SELECT n.name as network,
            k_int.descriptor as internal_descriptor, k_int.last_revealed as internal_last_revealed,
            k_ext.descriptor as external_descriptor, k_ext.last_revealed as external_last_revealed
            FROM network n
            LEFT JOIN keychain k_int ON n.wallet_name = k_int.wallet_name AND k_int.keychainkind = 'Internal'
            LEFT JOIN keychain k_ext ON n.wallet_name = k_ext.wallet_name AND k_ext.keychainkind = 'External'
            WHERE n.wallet_name = $1";

        // Fetch wallet data
        let row = sqlx::query(sql)
            .bind(&self.wallet_name)
            .fetch_optional(&mut *tx)
            .await
            .map_err(|e| StorageError::Sqlx(e.into()))?;

        if let Some(row) = row {
            Self::changeset_from_row(&mut tx, &mut changeset, row, &self.wallet_name).await?;
        }

        Ok(changeset)
    }

    pub(crate) async fn changeset_from_row(
        tx: &mut Transaction<'_, Postgres>,
        changeset: &mut ChangeSet,
        row: PgRow,
        wallet_name: &str,
    ) -> Result<(), StorageError> {
        let network: String = row.get("network");
        let internal_last_revealed: Option<i32> = row.get("internal_last_revealed");
        let external_last_revealed: Option<i32> = row.get("external_last_revealed");
        let internal_desc_str: Option<String> = row.get("internal_descriptor");
        let external_desc_str: Option<String> = row.get("external_descriptor");

        changeset.network = Some(Network::from_str(&network).expect("parse Network"));

        if let Some(desc_str) = external_desc_str {
            let descriptor: Descriptor<DescriptorPublicKey> = desc_str
                .parse()
                .map_err(|_| StorageError::Sqlx(SqlxError::Custom("parse descriptor".into())))?;
            let did = descriptor.descriptor_id();
            changeset.descriptor = Some(descriptor);
            if let Some(last_rev) = external_last_revealed {
                changeset.indexer.last_revealed.insert(did, last_rev as u32);
            }
        }

        if let Some(desc_str) = internal_desc_str {
            let descriptor: Descriptor<DescriptorPublicKey> = desc_str
                .parse()
                .map_err(|_| StorageError::Sqlx(SqlxError::Custom("parse descriptor".into())))?;
            let did = descriptor.descriptor_id();
            changeset.change_descriptor = Some(descriptor);
            if let Some(last_rev) = internal_last_revealed {
                changeset.indexer.last_revealed.insert(did, last_rev as u32);
            }
        }

        changeset.tx_graph = tx_graph_changeset_from_postgres(tx, wallet_name).await?;
        changeset.local_chain = local_chain_changeset_from_postgres(tx, wallet_name).await?;
        changeset.indexer.spk_cache = spk_cache_from_postgres(tx, wallet_name).await?;
        Ok(())
    }

    #[tracing::instrument(skip(self, changeset))]
    pub(crate) async fn write(&self, changeset: &ChangeSet) -> Result<(), StorageError> {
        if changeset.is_empty() {
            return Ok(());
        }
        log_info!(
            self.logger,
            "Writing changeset to postgres. num_blocks={}, num_txs={}, num_txouts={}, num_anchors={}",
            changeset.local_chain.blocks.len(),
            changeset.tx_graph.txs.len(),
            changeset.tx_graph.txouts.len(),
            changeset.tx_graph.anchors.len(),
        );

        let wallet_name = &self.wallet_name;
        let mut tx = self
            .pool
            .begin()
            .await
            .map_err(|e| StorageError::Sqlx(e.into()))?;

        if let Some(ref descriptor) = changeset.descriptor {
            insert_descriptor(&mut tx, wallet_name, descriptor, External)
                .await
                .map_err(StorageError::Sqlx)?;
        }

        if let Some(ref change_descriptor) = changeset.change_descriptor {
            insert_descriptor(&mut tx, wallet_name, change_descriptor, Internal)
                .await
                .map_err(StorageError::Sqlx)?;
        }

        if let Some(network) = changeset.network {
            insert_network(&mut tx, wallet_name, network)
                .await
                .map_err(StorageError::Sqlx)?;
        }

        let last_revealed_indices = &changeset.indexer.last_revealed;
        if !last_revealed_indices.is_empty() {
            for (desc_id, index) in last_revealed_indices {
                update_last_revealed(&mut tx, wallet_name, *desc_id, *index)
                    .await
                    .map_err(StorageError::Sqlx)?;
            }
        }

        spk_cache_persist_to_postgres(&mut tx, wallet_name, &changeset.indexer.spk_cache)
            .await
            .map_err(StorageError::Sqlx)?;

        local_chain_changeset_persist_to_postgres(&mut tx, wallet_name, &changeset.local_chain)
            .await
            .map_err(StorageError::Sqlx)?;
        tx_graph_changeset_persist_to_postgres(&mut tx, wallet_name, &changeset.tx_graph)
            .await
            .map_err(StorageError::Sqlx)?;

        tx.commit()
            .await
            .map_err(|e| StorageError::Sqlx(e.into()))?;

        Ok(())
    }
}

#[async_trait::async_trait]
impl Storage for PostgresStore {
    async fn initialize_bdk(&self) -> Result<ChangeSet, WalletError> {
        log_info!(
            self.logger,
            "Initializing storage for the BDK wallet. name={}",
            self.wallet_name
        );
        self.read()
            .await
            .map_err(|_| WalletError::StorageError("Did not initialize bdk storage".to_string()))
    }

    async fn persist_bdk(&self, changeset: &ChangeSet) -> Result<(), WalletError> {
        self.write(changeset)
            .await
            .map_err(|_| WalletError::StorageError("Did not persist bdk storage".to_string()))
    }
}

#[async_trait::async_trait]
impl ManagerStorage for PostgresStore {
    #[tracing::instrument(skip(self))]
    async fn get_contract(
        &self,
        id: &ddk_manager::ContractId,
    ) -> Result<Option<Contract>, ddk_manager::error::Error> {
        let contract =
            sqlx::query_as::<Postgres, ContractData>("SELECT * FROM contract_data WHERE id = $1")
                .bind(hex::encode(id))
                .fetch_optional(&self.pool)
                .await
                .map_err(to_storage_error)?;

        if let Some(contract) = contract {
            Ok(Some(deserialize_contract(&contract.contract_data)?))
        } else {
            Ok(None)
        }
    }

    #[tracing::instrument(skip(self))]
    async fn get_contracts(&self) -> Result<Vec<Contract>, ddk_manager::error::Error> {
        let contracts = sqlx::query_as::<Postgres, ContractData>("SELECT * FROM contract_data")
            .fetch_all(&self.pool)
            .await
            .map_err(to_storage_error)?;

        let contracts = contracts
            .into_iter()
            .map(|c| deserialize_contract(&c.contract_data))
            .collect::<Result<Vec<_>, _>>()?;

        Ok(contracts)
    }

    async fn create_contract(
        &self,
        contract: &OfferedContract,
    ) -> Result<(), ddk_manager::error::Error> {
        let mut tx = self.pool.begin().await.map_err(to_storage_error)?;
        let oracle_pubkey = contract.contract_info[0].oracle_announcements[0].oracle_public_key;
        let announcement_id = contract.contract_info[0].oracle_announcements[0]
            .oracle_event
            .event_id
            .clone();

        sqlx::query(
            r#"
           INSERT INTO contract_metadata (
               id, state, is_offer_party, counter_party,
               offer_collateral, accept_collateral, total_collateral, fee_rate_per_vb, 
               cet_locktime, refund_locktime, pnl, funding_txid, cet_txid, announcement_id, oracle_pubkey
           )
           VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15)
           "#,
        )
        .bind(hex::encode(contract.id))
        .bind(1_i16)
        .bind(contract.is_offer_party)
        .bind(hex::encode(contract.counter_party.serialize()))
        .bind(contract.offer_params.collateral.to_sat() as i64)
        .bind((contract.total_collateral - contract.offer_params.collateral).to_sat() as i64)
        .bind(contract.total_collateral.to_sat() as i64)
        .bind(contract.fee_rate_per_vb as i64)
        .bind(contract.cet_locktime as i32)
        .bind(contract.refund_locktime as i32)
        .bind(None as Option<i64>)
        .bind(None as Option<String>)
        .bind(None as Option<String>)
        .bind(announcement_id)
        .bind(oracle_pubkey.to_string())
        .execute(&mut *tx)
        .await
        .map_err(to_storage_error)?;

        sqlx::query(
            "INSERT INTO contract_data (id, state, contract_data, is_compressed) VALUES ($1, $2, $3, $4)"
        )
        .bind(hex::encode(contract.id))
        .bind(1_i16)
        .bind(serialize_contract(&Contract::Offered(contract.clone()))?)
        .bind(false)
        .execute(&mut *tx)
        .await
        .map_err(to_storage_error)?;

        tx.commit().await.map_err(to_storage_error)?;

        log_info!(
            self.logger,
            "Stored offered contract. id={}",
            hex::encode(contract.id)
        );

        Ok(())
    }

    #[tracing::instrument(skip(self))]
    async fn delete_contract(
        &self,
        id: &ddk_manager::ContractId,
    ) -> Result<(), ddk_manager::error::Error> {
        let mut tx = self.pool.begin().await.map_err(to_storage_error)?;
        let id = hex::encode(id);
        sqlx::query("DELETE FROM contract_data WHERE id = $1")
            .bind(id.clone())
            .execute(&mut *tx)
            .await
            .map_err(to_storage_error)?;

        sqlx::query("DELETE FROM contract_metadata WHERE id = $1")
            .bind(id)
            .execute(&mut *tx)
            .await
            .map_err(to_storage_error)?;

        tx.commit().await.map_err(to_storage_error)?;

        Ok(())
    }

    async fn update_contract(&self, contract: &Contract) -> Result<(), ddk_manager::error::Error> {
        log_info!(
            self.logger,
            "Updating contract. id={}",
            hex::encode(contract.get_id())
        );
        let prefix = ContractPrefix::get_prefix(contract);
        let contract_id = hex::encode(contract.get_id());
        let (offer_collateral, accept_collateral, total_collateral) = contract.get_collateral();

        // Start a transaction
        let mut tx = self.pool.begin().await.map_err(to_storage_error)?;

        // Step 1: Remove by temp_id if Accepted or Signed
        match contract {
            a @ Contract::Accepted(_) | a @ Contract::Signed(_) => {
                log_info!(
                    self.logger,
                    "Deleting contract by temp_id. tmp_id={}",
                    hex::encode(a.get_temporary_id())
                );
                let temp_id = hex::encode(a.get_temporary_id());
                sqlx::query("DELETE FROM contract_data WHERE id = $1")
                    .bind(temp_id.clone())
                    .execute(&mut *tx)
                    .await
                    .map_err(to_storage_error)?;
                sqlx::query("DELETE FROM contract_metadata WHERE id = $1")
                    .bind(temp_id)
                    .execute(&mut *tx)
                    .await
                    .map_err(to_storage_error)?;
            }
            _ => {}
        }

        let funding_txid = contract.get_funding_txid().map(|txid| txid.to_string());
        let cet_txid = contract.get_cet_txid().map(|txid| txid.to_string());
        let oracle_pubkey = contract
            .get_oracle_announcement()
            .map(|ann| ann.oracle_public_key.to_string());
        let announcement_id = contract
            .get_oracle_announcement()
            .map(|ann| ann.oracle_event.event_id.clone());

        // A single atomic upsert: the read-modify-write it replaces raced under
        // concurrent updates, and its insert arm hardcoded is_offer_party and
        // fee_rate_per_vb. The update arm deliberately leaves the columns set
        // at insert time untouched and only advances the mutable ones.
        sqlx::query(
            r#"
            INSERT INTO contract_metadata (
                id, state, is_offer_party, counter_party,
                offer_collateral, accept_collateral, total_collateral, fee_rate_per_vb,
                cet_locktime, refund_locktime, pnl, funding_txid, cet_txid, announcement_id, oracle_pubkey
            )
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15)
            ON CONFLICT (id) DO UPDATE SET
                state = EXCLUDED.state,
                pnl = EXCLUDED.pnl,
                funding_txid = COALESCE(EXCLUDED.funding_txid, contract_metadata.funding_txid),
                cet_txid = COALESCE(EXCLUDED.cet_txid, contract_metadata.cet_txid)
            "#,
        )
        .bind(&contract_id)
        .bind(prefix as i16)
        .bind(contract.is_offer_party())
        .bind(hex::encode(contract.get_counter_party_id().serialize()))
        .bind(offer_collateral.to_sat() as i64)
        .bind(accept_collateral.to_sat() as i64)
        .bind(total_collateral.to_sat() as i64)
        .bind(contract.get_fee_rate_per_vb() as i64)
        .bind(contract.get_cet_locktime() as i32)
        .bind(contract.get_refund_locktime() as i32)
        .bind(Some(contract.get_pnl().to_sat()))
        .bind(&funding_txid)
        .bind(&cet_txid)
        .bind(announcement_id.unwrap_or_else(|| "legacy_data".to_string()))
        .bind(oracle_pubkey.unwrap_or_else(|| "legacy_data".to_string()))
        .execute(&mut *tx)
        .await
        .map_err(to_storage_error)?;

        let serialized_contract = serialize_contract(contract)?;

        sqlx::query(
            "INSERT INTO contract_data (id, state, contract_data, is_compressed)
             VALUES ($1, $2, $3, $4)
             ON CONFLICT (id) DO UPDATE SET
                 state = EXCLUDED.state,
                 contract_data = EXCLUDED.contract_data",
        )
        .bind(&contract_id)
        .bind(prefix as i16)
        .bind(&serialized_contract)
        .bind(false)
        .execute(&mut *tx)
        .await
        .map_err(to_storage_error)?;

        tx.commit().await.map_err(to_storage_error)?;

        Ok(())
    }

    #[tracing::instrument(skip(self))]
    async fn get_signed_contracts(&self) -> Result<Vec<SignedContract>, ddk_manager::error::Error> {
        let contracts =
            sqlx::query_as::<Postgres, ContractData>("SELECT * FROM contract_data WHERE state = 3")
                .fetch_all(&self.pool)
                .await
                .map_err(to_storage_error)?;

        let signed = contracts
            .into_iter()
            .map(|c| {
                let mut cursor = lightning::io::Cursor::new(&c.contract_data);
                cursor.set_position(cursor.position() + 1);
                SignedContract::deserialize(&mut cursor).map_err(to_storage_error)
            })
            .collect::<Result<Vec<_>, ddk_manager::error::Error>>()?;

        Ok(signed)
    }

    #[tracing::instrument(skip(self))]
    async fn get_contract_offers(&self) -> Result<Vec<OfferedContract>, ddk_manager::error::Error> {
        let contracts = sqlx::query_as::<Postgres, ContractData>(
            "SELECT cd.id, cd.state, cd.contract_data, cd.is_compressed 
         FROM contract_data cd
         INNER JOIN contract_metadata cm ON cd.id = cm.id
         WHERE cm.state = 1 AND cm.is_offer_party = false",
        )
        .fetch_all(&self.pool)
        .await
        .map_err(to_storage_error)?;

        let offers = contracts
            .into_iter()
            .map(|c| {
                let mut cursor = lightning::io::Cursor::new(&c.contract_data);
                cursor.set_position(cursor.position() + 1);
                OfferedContract::deserialize(&mut cursor).map_err(to_storage_error)
            })
            .collect::<Result<Vec<_>, ddk_manager::error::Error>>()?;

        Ok(offers)
    }

    #[tracing::instrument(skip(self))]
    async fn get_confirmed_contracts(
        &self,
    ) -> Result<Vec<SignedContract>, ddk_manager::error::Error> {
        let contracts =
            sqlx::query_as::<Postgres, ContractData>("SELECT * FROM contract_data WHERE state = 4")
                .fetch_all(&self.pool)
                .await
                .map_err(to_storage_error)?;

        let signed = contracts
            .into_iter()
            .map(|c| {
                let mut cursor = lightning::io::Cursor::new(&c.contract_data);
                cursor.set_position(cursor.position() + 1);
                SignedContract::deserialize(&mut cursor).map_err(to_storage_error)
            })
            .collect::<Result<Vec<_>, ddk_manager::error::Error>>()?;

        Ok(signed)
    }

    #[tracing::instrument(skip(self))]
    async fn get_preclosed_contracts(
        &self,
    ) -> Result<Vec<PreClosedContract>, ddk_manager::error::Error> {
        let contracts =
            sqlx::query_as::<Postgres, ContractData>("SELECT * FROM contract_data WHERE state = 5")
                .fetch_all(&self.pool)
                .await
                .map_err(to_storage_error)?;

        let preclosed = contracts
            .into_iter()
            .map(|c| {
                let mut cursor = lightning::io::Cursor::new(&c.contract_data);
                cursor.set_position(cursor.position() + 1);
                PreClosedContract::deserialize(&mut cursor).map_err(to_storage_error)
            })
            .collect::<Result<Vec<_>, ddk_manager::error::Error>>()?;

        Ok(preclosed)
    }

    #[tracing::instrument(skip(self))]
    async fn upsert_channel(
        &self,
        _channel: ddk_manager::channel::Channel,
        _contract: Option<Contract>,
    ) -> Result<(), ddk_manager::error::Error> {
        unimplemented!("Channels not supported.")
    }

    #[tracing::instrument(skip(self))]
    async fn delete_channel(
        &self,
        _channel_id: &ddk_manager::ChannelId,
    ) -> Result<(), ddk_manager::error::Error> {
        unimplemented!("Channels not supported.")
    }

    #[tracing::instrument(skip(self, _channel_state))]
    async fn get_signed_channels(
        &self,
        _channel_state: Option<ddk_manager::channel::signed_channel::SignedChannelStateType>,
    ) -> Result<Vec<ddk_manager::channel::signed_channel::SignedChannel>, ddk_manager::error::Error>
    {
        unimplemented!("Channels not supported.")
    }

    #[tracing::instrument(skip(self))]
    async fn get_channel(
        &self,
        _channel_id: &ddk_manager::ChannelId,
    ) -> Result<Option<ddk_manager::channel::Channel>, ddk_manager::error::Error> {
        unimplemented!("Channels not supported.")
    }

    #[tracing::instrument(skip(self))]
    async fn get_offered_channels(
        &self,
    ) -> Result<Vec<ddk_manager::channel::offered_channel::OfferedChannel>, ddk_manager::error::Error>
    {
        unimplemented!("Channels not supported.")
    }

    #[tracing::instrument(skip(self))]
    async fn persist_chain_monitor(
        &self,
        _monitor: &ddk_manager::chain_monitor::ChainMonitor,
    ) -> Result<(), ddk_manager::error::Error> {
        unimplemented!("Chain monitor not supported.")
    }

    #[tracing::instrument(skip(self))]
    async fn get_chain_monitor(
        &self,
    ) -> Result<Option<ddk_manager::chain_monitor::ChainMonitor>, ddk_manager::error::Error> {
        Ok(None)
    }
}

/// Insert keychain descriptors.
#[tracing::instrument(skip_all)]
async fn insert_descriptor(
    tx: &mut Transaction<'_, Postgres>,
    wallet_name: &str,
    descriptor: &ExtendedDescriptor,
    keychain: KeychainKind,
) -> Result<(), SqlxError> {
    let descriptor_str = descriptor.to_string();

    let descriptor_id = descriptor.descriptor_id().to_byte_array();
    let keychain = match keychain {
        External => "External",
        Internal => "Internal",
    };

    sqlx::query(
        "INSERT INTO keychain (wallet_name, keychainkind, descriptor, descriptor_id) VALUES ($1, $2, $3, $4)",
    )
        .bind(wallet_name)
        .bind(keychain)
        .bind(descriptor_str)
        .bind(descriptor_id.as_slice())
        .execute(&mut **tx)
        .await?;

    Ok(())
}

/// Insert network.
#[tracing::instrument(skip(tx))]
async fn insert_network(
    tx: &mut Transaction<'_, Postgres>,
    wallet_name: &str,
    network: Network,
) -> Result<(), SqlxError> {
    sqlx::query("INSERT INTO network (wallet_name, name) VALUES ($1, $2)")
        .bind(wallet_name)
        .bind(network.to_string())
        .execute(&mut **tx)
        .await?;

    Ok(())
}

/// Update keychain last revealed
#[tracing::instrument(skip(tx))]
async fn update_last_revealed(
    tx: &mut Transaction<'_, Postgres>,
    wallet_name: &str,
    descriptor_id: DescriptorId,
    last_revealed: u32,
) -> Result<(), SqlxError> {
    // BDK's merge rule for last_revealed keeps the greater index; a stale
    // write must never regress it or the wallet re-reveals used addresses.
    sqlx::query(
        "UPDATE keychain SET last_revealed = GREATEST(last_revealed, $1)
         WHERE wallet_name = $2 AND descriptor_id = $3",
    )
    .bind(last_revealed as i32)
    .bind(wallet_name)
    .bind(descriptor_id.to_byte_array())
    .execute(&mut **tx)
    .await?;

    Ok(())
}

/// Select transactions, txouts, and anchors.
#[tracing::instrument(skip(db_tx))]
async fn tx_graph_changeset_from_postgres(
    db_tx: &mut Transaction<'_, Postgres>,
    wallet_name: &str,
) -> Result<tx_graph::ChangeSet<ConfirmationBlockTime>, SqlxError> {
    let mut changeset = tx_graph::ChangeSet::default();

    // Fetch transactions
    let rows = sqlx::query(
        "SELECT txid, whole_tx, last_seen, first_seen, last_evicted FROM tx WHERE wallet_name = $1",
    )
    .bind(wallet_name)
    .fetch_all(&mut **db_tx)
    .await?;

    for row in rows {
        let txid: String = row.get("txid");
        let txid = Txid::from_str(&txid)?;
        let whole_tx: Option<Vec<u8>> = row.get("whole_tx");
        let last_seen: Option<i64> = row.get("last_seen");
        let first_seen: Option<i64> = row.get("first_seen");
        let last_evicted: Option<i64> = row.get("last_evicted");

        if let Some(tx_bytes) = whole_tx {
            if let Ok(tx) = bitcoin::Transaction::consensus_decode(&mut tx_bytes.as_slice()) {
                changeset.txs.insert(Arc::new(tx));
            }
        }
        if let Some(last_seen) = last_seen {
            changeset.last_seen.insert(txid, last_seen as u64);
        }
        if let Some(first_seen) = first_seen {
            changeset.first_seen.insert(txid, first_seen as u64);
        }
        if let Some(last_evicted) = last_evicted {
            changeset.last_evicted.insert(txid, last_evicted as u64);
        }
    }

    // Fetch txouts
    let rows = sqlx::query("SELECT txid, vout, value, script FROM txout WHERE wallet_name = $1")
        .bind(wallet_name)
        .fetch_all(&mut **db_tx)
        .await?;

    for row in rows {
        let txid: String = row.get("txid");
        let txid = Txid::from_str(&txid)?;
        let vout: i32 = row.get("vout");
        let value: i64 = row.get("value");
        let script: Vec<u8> = row.get("script");

        changeset.txouts.insert(
            OutPoint {
                txid,
                vout: vout as u32,
            },
            TxOut {
                value: Amount::from_sat(value as u64),
                script_pubkey: ScriptBuf::from(script),
            },
        );
    }

    // Fetch anchors
    let rows = sqlx::query("SELECT anchor, txid FROM anchor_tx WHERE wallet_name = $1")
        .bind(wallet_name)
        .fetch_all(&mut **db_tx)
        .await?;

    for row in rows {
        let anchor: serde_json::Value = row.get("anchor");
        let txid: String = row.get("txid");
        let txid = Txid::from_str(&txid)?;

        if let Ok(anchor) = serde_json::from_value::<ConfirmationBlockTime>(anchor) {
            changeset.anchors.insert((anchor, txid));
        }
    }

    Ok(changeset)
}

/// Insert transactions, txouts, and anchors.
#[tracing::instrument(skip(db_tx, changeset))]
async fn tx_graph_changeset_persist_to_postgres(
    db_tx: &mut Transaction<'_, Postgres>,
    wallet_name: &str,
    changeset: &tx_graph::ChangeSet<ConfirmationBlockTime>,
) -> Result<(), SqlxError> {
    for tx in &changeset.txs {
        sqlx::query(
            "INSERT INTO tx (wallet_name, txid, whole_tx) VALUES ($1, $2, $3)
             ON CONFLICT (wallet_name, txid) DO UPDATE SET whole_tx = $3",
        )
        .bind(wallet_name)
        .bind(tx.compute_txid().to_string())
        .bind(consensus::serialize(tx.as_ref()))
        .execute(&mut **db_tx)
        .await?;
    }

    // A last_seen entry can arrive before the row for its txid exists; a plain
    // UPDATE silently dropped it. last_seen only ever increases.
    for (&txid, &last_seen) in &changeset.last_seen {
        sqlx::query(
            "INSERT INTO tx (wallet_name, txid, last_seen) VALUES ($1, $2, $3)
             ON CONFLICT (wallet_name, txid)
             DO UPDATE SET last_seen = GREATEST(tx.last_seen, EXCLUDED.last_seen)",
        )
        .bind(wallet_name)
        .bind(txid.to_string())
        .bind(last_seen as i64)
        .execute(&mut **db_tx)
        .await?;
    }

    // first_seen only ever decreases and last_evicted only ever increases,
    // matching the tx_graph merge rules. LEAST/GREATEST ignore NULL.
    for (&txid, &first_seen) in &changeset.first_seen {
        sqlx::query(
            "INSERT INTO tx (wallet_name, txid, first_seen) VALUES ($1, $2, $3)
             ON CONFLICT (wallet_name, txid)
             DO UPDATE SET first_seen = LEAST(tx.first_seen, EXCLUDED.first_seen)",
        )
        .bind(wallet_name)
        .bind(txid.to_string())
        .bind(first_seen as i64)
        .execute(&mut **db_tx)
        .await?;
    }

    for (&txid, &last_evicted) in &changeset.last_evicted {
        sqlx::query(
            "INSERT INTO tx (wallet_name, txid, last_evicted) VALUES ($1, $2, $3)
             ON CONFLICT (wallet_name, txid)
             DO UPDATE SET last_evicted = GREATEST(tx.last_evicted, EXCLUDED.last_evicted)",
        )
        .bind(wallet_name)
        .bind(txid.to_string())
        .bind(last_evicted as i64)
        .execute(&mut **db_tx)
        .await?;
    }

    for (op, txo) in &changeset.txouts {
        sqlx::query(
            "INSERT INTO txout (wallet_name, txid, vout, value, script) VALUES ($1, $2, $3, $4, $5)
             ON CONFLICT (wallet_name, txid, vout) DO UPDATE SET value = $4, script = $5",
        )
        .bind(wallet_name)
        .bind(op.txid.to_string())
        .bind(op.vout as i32)
        .bind(txo.value.to_sat() as i64)
        .bind(txo.script_pubkey.as_bytes())
        .execute(&mut **db_tx)
        .await?;
    }

    for (anchor, txid) in &changeset.anchors {
        let block_hash = anchor.anchor_block().hash;
        let anchor = serde_json::to_value(anchor)?;
        sqlx::query(
            "INSERT INTO anchor_tx (wallet_name, block_hash, anchor, txid) VALUES ($1, $2, $3, $4)
             ON CONFLICT (wallet_name, block_hash, txid) DO UPDATE SET anchor = $3",
        )
        .bind(wallet_name)
        .bind(block_hash.to_string())
        .bind(anchor)
        .bind(txid.to_string())
        .execute(&mut **db_tx)
        .await?;
    }

    Ok(())
}

/// Select the cached script pubkeys of the keychain indexer.
#[tracing::instrument(skip(db_tx))]
async fn spk_cache_from_postgres(
    db_tx: &mut Transaction<'_, Postgres>,
    wallet_name: &str,
) -> Result<BTreeMap<DescriptorId, BTreeMap<u32, ScriptBuf>>, SqlxError> {
    let mut cache: BTreeMap<DescriptorId, BTreeMap<u32, ScriptBuf>> = BTreeMap::new();

    let rows =
        sqlx::query("SELECT descriptor_id, spk_index, script FROM spk_cache WHERE wallet_name = $1")
            .bind(wallet_name)
            .fetch_all(&mut **db_tx)
            .await?;

    for row in rows {
        let descriptor_id: Vec<u8> = row.get("descriptor_id");
        let spk_index: i32 = row.get("spk_index");
        let script: Vec<u8> = row.get("script");
        let descriptor_id = <[u8; 32]>::try_from(descriptor_id.as_slice())
            .map_err(|_| SqlxError::Custom("descriptor_id is not 32 bytes".into()))?;
        cache
            .entry(DescriptorId(sha256::Hash::from_byte_array(descriptor_id)))
            .or_default()
            .insert(spk_index as u32, ScriptBuf::from(script));
    }

    Ok(cache)
}

/// Insert cached script pubkeys of the keychain indexer.
#[tracing::instrument(skip_all)]
async fn spk_cache_persist_to_postgres(
    db_tx: &mut Transaction<'_, Postgres>,
    wallet_name: &str,
    spk_cache: &BTreeMap<DescriptorId, BTreeMap<u32, ScriptBuf>>,
) -> Result<(), SqlxError> {
    for (descriptor_id, spks) in spk_cache {
        let descriptor_id = descriptor_id.to_byte_array();
        for (spk_index, script) in spks {
            sqlx::query(
                "INSERT INTO spk_cache (wallet_name, descriptor_id, spk_index, script)
                 VALUES ($1, $2, $3, $4)
                 ON CONFLICT (wallet_name, descriptor_id, spk_index) DO NOTHING",
            )
            .bind(wallet_name)
            .bind(descriptor_id.as_slice())
            .bind(*spk_index as i32)
            .bind(script.as_bytes())
            .execute(&mut **db_tx)
            .await?;
        }
    }

    Ok(())
}

/// Select blocks.
#[tracing::instrument(skip(db_tx))]
async fn local_chain_changeset_from_postgres(
    db_tx: &mut Transaction<'_, Postgres>,
    wallet_name: &str,
) -> Result<local_chain::ChangeSet, SqlxError> {
    let mut changeset = local_chain::ChangeSet::default();

    let rows = sqlx::query("SELECT hash, height FROM block WHERE wallet_name = $1")
        .bind(wallet_name)
        .fetch_all(&mut **db_tx)
        .await?;

    for row in rows {
        let hash: String = row.get("hash");
        let height: i32 = row.get("height");
        let block_hash = BlockHash::from_str(&hash)?;
        changeset.blocks.insert(height as u32, Some(block_hash));
    }

    Ok(changeset)
}

/// Insert blocks.
#[tracing::instrument(skip(db_tx, changeset))]
async fn local_chain_changeset_persist_to_postgres(
    db_tx: &mut Transaction<'_, Postgres>,
    wallet_name: &str,
    changeset: &local_chain::ChangeSet,
) -> Result<(), SqlxError> {
    for (&height, &hash) in &changeset.blocks {
        match hash {
            Some(hash) => {
                sqlx::query(
                    "INSERT INTO block (wallet_name, hash, height) VALUES ($1, $2, $3)
                     ON CONFLICT (wallet_name, height) DO UPDATE SET hash = EXCLUDED.hash",
                )
                .bind(wallet_name)
                .bind(hash.to_string())
                .bind(height as i32)
                .execute(&mut **db_tx)
                .await?;
            }
            None => {
                sqlx::query("DELETE FROM block WHERE wallet_name = $1 AND height = $2")
                    .bind(wallet_name)
                    .bind(height as i32)
                    .execute(&mut **db_tx)
                    .await?;
            }
        }
    }

    Ok(())
}

/// Collects information on all the wallets in the database and dumps it to stdout.
#[tracing::instrument(skip(db))]
#[allow(dead_code)]
async fn easy_backup(db: Pool<Postgres>, logger: Arc<Logger>) -> Result<(), SqlxError> {
    log_info!(logger, "Starting backup of the wallet database");

    let statement = "SELECT * FROM keychain";

    let results = sqlx::query_as::<_, KeychainEntry>(statement)
        .fetch_all(&db)
        .await?;

    let json_array = json!(results);
    println!("{}", serde_json::to_string_pretty(&json_array)?);

    log_info!(logger, "Wallet database backup completed successfully.");
    Ok(())
}

/// Represents a row in the keychain table.
#[derive(serde::Serialize, FromRow)]
#[allow(dead_code)]
struct KeychainEntry {
    wallet_name: String,
    keychainkind: String,
    descriptor: String,
    descriptor_id: Vec<u8>,
    last_revealed: i32,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{logger::LogLevel, util::ser::deserialize_contract};
    use ddk_manager::Storage;
    use ddk_testenv::postgres::TestPostgres;

    /// Returns the store alongside the server backing it: the server stops when
    /// dropped, so the caller has to keep it alive.
    async fn seed_db() -> (TestPostgres, PostgresStore) {
        let server = TestPostgres::start("ddk").await;
        let store = PostgresStore::new(
            server.url(),
            true,
            Arc::new(Logger::console(
                "console_logger".to_string(),
                LogLevel::Info,
            )),
            "test".to_string(),
        )
        .await
        .unwrap();

        let offered = include_bytes!("../../../../testconfig/contract_binaries/Offered");
        let offered_contract = deserialize_contract(&offered.to_vec()).unwrap();
        match offered_contract {
            Contract::Offered(offered_contract) => {
                store
                    .create_contract(&offered_contract)
                    .await
                    .expect("Failed to create offered contract");
            }
            _ => panic!("Offered contract is not an OfferedContract"),
        }
        let accept = include_bytes!("../../../../testconfig/contract_binaries/Accepted");
        let accepted_contract = deserialize_contract(&accept.to_vec()).unwrap();
        store
            .update_contract(&accepted_contract)
            .await
            .expect("Failed to update accepted contract");
        let signed = include_bytes!("../../../../testconfig/contract_binaries/Signed");
        let signed_contract = deserialize_contract(&signed.to_vec()).unwrap();
        store
            .update_contract(&signed_contract)
            .await
            .expect("Failed to update signed contract");
        let confirmed = include_bytes!("../../../../testconfig/contract_binaries/Confirmed");
        let confirmed_contract = deserialize_contract(&confirmed.to_vec()).unwrap();
        store
            .update_contract(&confirmed_contract)
            .await
            .expect("Failed to update confirmed contract");
        let preclosed = include_bytes!("../../../../testconfig/contract_binaries/PreClosed");
        let preclosed_contract = deserialize_contract(&preclosed.to_vec()).unwrap();
        store
            .update_contract(&preclosed_contract)
            .await
            .expect("Failed to update preclosed contract");

        let closed = include_bytes!("../../../../testconfig/contract_binaries/Closed");
        let closed_contract = deserialize_contract(&closed.to_vec()).unwrap();
        store
            .update_contract(&closed_contract)
            .await
            .expect("Failed to update closed contract");

        (server, store)
    }

    fn dummy_tx() -> bitcoin::Transaction {
        bitcoin::Transaction {
            version: bitcoin::transaction::Version::TWO,
            lock_time: bitcoin::absolute::LockTime::ZERO,
            input: vec![],
            output: vec![],
        }
    }

    #[tokio::test]
    async fn postgres() {
        let (_server, db) = seed_db().await;

        let confirmed_rows = db.get_contract_metadata(None).await.unwrap();
        assert_eq!(confirmed_rows.len(), 1);
        assert_eq!(confirmed_rows[0].state, ContractPrefix::Closed as i16);
        let contracts = db.get_contracts().await.unwrap();
        assert!(contracts.len() > 0);
    }

    #[tokio::test]
    async fn last_revealed_never_regresses() {
        let (_server, db) = seed_db().await;

        let descriptor: ExtendedDescriptor = "wpkh([73c5da0a/84'/1'/0']tpubDC8msFGeGuwnKG9Upg7DM2b4DaRqg3CUZa5g8v2SRQ6K4NSkxUgd7HsL2XVWbVm39yBA4LAxysQAm397zwQSQoQgewGiYZqrA9DsP4zbQ1M/0/*)"
            .parse()
            .unwrap();
        let did = descriptor.descriptor_id();

        let mut changeset = ChangeSet::default();
        changeset.network = Some(Network::Regtest);
        changeset.descriptor = Some(descriptor);
        changeset.indexer.last_revealed.insert(did, 7);
        db.write(&changeset).await.unwrap();

        // A stale write with a smaller index must not regress the value.
        let mut stale = ChangeSet::default();
        stale.indexer.last_revealed.insert(did, 3);
        db.write(&stale).await.unwrap();
        let read = db.read().await.unwrap();
        assert_eq!(read.indexer.last_revealed.get(&did), Some(&7));

        // A greater index still advances it.
        let mut advance = ChangeSet::default();
        advance.indexer.last_revealed.insert(did, 9);
        db.write(&advance).await.unwrap();
        let read = db.read().await.unwrap();
        assert_eq!(read.indexer.last_revealed.get(&did), Some(&9));
    }

    #[tokio::test]
    async fn block_reorg_replaces_hash_at_height() {
        let (_server, db) = seed_db().await;

        let hash_a = BlockHash::from_byte_array([0xAA; 32]);
        let hash_b = BlockHash::from_byte_array([0xBB; 32]);

        let mut changeset = ChangeSet::default();
        changeset.network = Some(Network::Regtest);
        changeset.local_chain.blocks.insert(100, Some(hash_a));
        db.write(&changeset).await.unwrap();

        // A reorg replaces the hash at the same height; the old row must go.
        let mut reorg = ChangeSet::default();
        reorg.local_chain.blocks.insert(100, Some(hash_b));
        db.write(&reorg).await.unwrap();

        let read = db.read().await.unwrap();
        assert_eq!(read.local_chain.blocks.get(&100), Some(&Some(hash_b)));
        assert_eq!(read.local_chain.blocks.len(), 1);

        // An anchored tx must not block removing the block row.
        let tx = dummy_tx();
        let mut anchor = ChangeSet::default();
        anchor.tx_graph.txs.insert(Arc::new(tx.clone()));
        anchor.tx_graph.anchors.insert((
            ConfirmationBlockTime {
                block_id: bdk_chain::BlockId {
                    height: 100,
                    hash: hash_b,
                },
                confirmation_time: 1234,
            },
            tx.compute_txid(),
        ));
        db.write(&anchor).await.unwrap();

        let mut remove = ChangeSet::default();
        remove.local_chain.blocks.insert(100, None);
        db.write(&remove).await.unwrap();

        let read = db.read().await.unwrap();
        assert!(read.local_chain.blocks.get(&100).is_none());
    }

    #[tokio::test]
    async fn update_contract_inserts_real_metadata() {
        let (_server, db) = seed_db().await;

        // The metadata row was recreated by update_contract after the temp-id
        // delete (the Accepted transition); it must carry the contract's real
        // values instead of hardcoded ones.
        let accept = include_bytes!("../../../../testconfig/contract_binaries/Accepted");
        let accepted_contract = deserialize_contract(&accept.to_vec()).unwrap();

        let metadata = db.get_contract_metadata(None).await.unwrap();
        assert_eq!(metadata.len(), 1);
        assert_eq!(metadata[0].is_offer_party, accepted_contract.is_offer_party());
        assert_eq!(
            metadata[0].fee_rate_per_vb as u64,
            accepted_contract.get_fee_rate_per_vb()
        );
    }

    #[tokio::test]
    async fn last_seen_survives_missing_tx_row() {
        let (_server, db) = seed_db().await;

        let txid = dummy_tx().compute_txid();

        // No tx row exists yet for this txid; the value must not be dropped.
        let mut changeset = ChangeSet::default();
        changeset.network = Some(Network::Regtest);
        changeset.tx_graph.last_seen.insert(txid, 100);
        db.write(&changeset).await.unwrap();

        let read = db.read().await.unwrap();
        assert_eq!(read.tx_graph.last_seen.get(&txid), Some(&100));

        // last_seen only ever increases.
        let mut stale = ChangeSet::default();
        stale.tx_graph.last_seen.insert(txid, 50);
        db.write(&stale).await.unwrap();
        let read = db.read().await.unwrap();
        assert_eq!(read.tx_graph.last_seen.get(&txid), Some(&100));
    }

    #[tokio::test]
    async fn tx_timestamps_and_spk_cache_roundtrip() {
        let (_server, db) = seed_db().await;

        let tx = dummy_tx();
        let txid = tx.compute_txid();
        let did = DescriptorId(sha256::Hash::from_byte_array([0x11; 32]));
        let script = ScriptBuf::from(vec![0x00, 0x14]);

        let mut changeset = ChangeSet::default();
        changeset.network = Some(Network::Regtest);
        changeset.tx_graph.txs.insert(Arc::new(tx));
        changeset.tx_graph.first_seen.insert(txid, 100);
        changeset.tx_graph.last_evicted.insert(txid, 200);
        changeset
            .indexer
            .spk_cache
            .entry(did)
            .or_default()
            .insert(5, script.clone());
        db.write(&changeset).await.unwrap();

        let read = db.read().await.unwrap();
        assert_eq!(read.tx_graph.first_seen.get(&txid), Some(&100));
        assert_eq!(read.tx_graph.last_evicted.get(&txid), Some(&200));
        assert_eq!(
            read.indexer.spk_cache.get(&did).and_then(|m| m.get(&5)),
            Some(&script)
        );

        // Merge rules: first_seen only decreases, last_evicted only increases.
        let mut ignored = ChangeSet::default();
        ignored.tx_graph.first_seen.insert(txid, 150);
        ignored.tx_graph.last_evicted.insert(txid, 150);
        db.write(&ignored).await.unwrap();
        let read = db.read().await.unwrap();
        assert_eq!(read.tx_graph.first_seen.get(&txid), Some(&100));
        assert_eq!(read.tx_graph.last_evicted.get(&txid), Some(&200));

        let mut taken = ChangeSet::default();
        taken.tx_graph.first_seen.insert(txid, 50);
        taken.tx_graph.last_evicted.insert(txid, 250);
        db.write(&taken).await.unwrap();
        let read = db.read().await.unwrap();
        assert_eq!(read.tx_graph.first_seen.get(&txid), Some(&50));
        assert_eq!(read.tx_graph.last_evicted.get(&txid), Some(&250));
    }

    #[tokio::test]
    async fn delete_contract_removes_rows() {
        let (_server, db) = seed_db().await;

        let contracts = db.get_contracts().await.unwrap();
        let id = contracts[0].get_id();

        db.delete_contract(&id)
            .await
            .expect("delete_contract should succeed");

        assert!(db.get_contract(&id).await.unwrap().is_none());
        assert!(db.get_contract_metadata(None).await.unwrap().is_empty());
    }
}

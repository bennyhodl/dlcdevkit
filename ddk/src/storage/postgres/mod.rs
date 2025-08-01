use super::sqlx::{ContractData, ContractMetadata, SqlxError};
use crate::error::{StorageError, WalletError};
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
    hashes::Hash,
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
use std::str::FromStr;
use std::sync::Arc;
use tracing::info;

/// Manages a pool of database connections.
#[derive(Debug)]
pub struct PostgresStore {
    pub(crate) pool: Pool<Postgres>,
    wallet_name: String,
}

impl PostgresStore {
    pub async fn new(
        url: &str,
        migrations: bool,
        wallet_name: String,
    ) -> Result<Self, StorageError> {
        let pool = PoolOptions::<Postgres>::new()
            .max_connections(5)
            .connect(url)
            .await
            .map_err(|e| StorageError::Sqlx(e.into()))?;
        if migrations {
            tracing::info!("Migrating postgres");
            sqlx::migrate!("src/storage/postgres/migrations")
                .run(&pool)
                .await
                .map_err(|e| StorageError::Sqlx(e.into()))?;
        }

        Ok(Self { pool, wallet_name })
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

    #[tracing::instrument]
    pub(crate) async fn read(&self) -> Result<ChangeSet, StorageError> {
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

    #[tracing::instrument]
    pub(crate) async fn changeset_from_row(
        tx: &mut Transaction<'_, Postgres>,
        changeset: &mut ChangeSet,
        row: PgRow,
        wallet_name: &str,
    ) -> Result<(), StorageError> {
        tracing::info!("changeset from row");

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
        Ok(())
    }

    #[tracing::instrument]
    pub(crate) async fn write(&self, changeset: &ChangeSet) -> Result<(), StorageError> {
        tracing::info!("changeset write");
        if changeset.is_empty() {
            return Ok(());
        }

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
        tracing::info!("initialize store");
        self.read()
            .await
            .map_err(|_| WalletError::StorageError("Did not initialize bdk storage".to_string()))
    }

    async fn persist_bdk(&self, changeset: &ChangeSet) -> Result<(), WalletError> {
        tracing::info!("persist store");

        self.write(changeset)
            .await
            .map_err(|_| WalletError::StorageError("Did not persist bdk storage".to_string()))
    }
}

#[async_trait::async_trait]
impl ManagerStorage for PostgresStore {
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

        println!(
            "inserting contract metadata {}{}",
            oracle_pubkey, announcement_id
        );
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

        Ok(())
    }

    async fn delete_contract(
        &self,
        id: &ddk_manager::ContractId,
    ) -> Result<(), ddk_manager::error::Error> {
        let mut tx = self.pool.begin().await.map_err(to_storage_error)?;
        let id = hex::encode(id);
        sqlx::query_as::<Postgres, ContractMetadata>("DELETE FROM contract_metadata WHERE id = $1")
            .bind(id.clone())
            .fetch_one(&mut *tx)
            .await
            .map_err(to_storage_error)?;

        sqlx::query_as::<Postgres, ContractData>("DELETE FROM contract_data WHERE id = $1")
            .bind(id)
            .fetch_one(&mut *tx)
            .await
            .map_err(to_storage_error)?;

        tx.commit().await.map_err(to_storage_error)?;

        Ok(())
    }

    async fn update_contract(&self, contract: &Contract) -> Result<(), ddk_manager::error::Error> {
        tracing::info!("Updating contract: {}", hex::encode(contract.get_id()));
        let prefix = ContractPrefix::get_prefix(contract);
        let contract_id = hex::encode(contract.get_id());
        let (offer_collateral, accept_collateral, total_collateral) = contract.get_collateral();

        // Start a transaction
        let mut tx = self.pool.begin().await.map_err(to_storage_error)?;

        // Step 1: Remove by temp_id if Accepted or Signed
        match contract {
            a @ Contract::Accepted(_) | a @ Contract::Signed(_) => {
                tracing::info!(
                    "Deleting contract by temp_id: {:?}",
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

        let existing_metadata = sqlx::query_as::<Postgres, ContractMetadata>(
            "SELECT * FROM contract_metadata WHERE id = $1",
        )
        .bind(&contract_id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(to_storage_error)?;

        if existing_metadata.is_some() {
            sqlx::query(
                r#"
            UPDATE contract_metadata SET
                state = $2,
                pnl = $3,
                funding_txid = COALESCE($4, funding_txid),
                cet_txid = COALESCE($5, cet_txid)
            WHERE id = $1
            "#,
            )
            .bind(&contract_id)
            .bind(prefix as i16)
            .bind(Some(contract.get_pnl().to_sat()))
            .bind(&funding_txid)
            .bind(&cet_txid)
            .execute(&mut *tx)
            .await
            .map_err(to_storage_error)?;
        } else {
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
            .bind(&contract_id)
            .bind(prefix as i16)
            // need to track this
            .bind(false)
            .bind(hex::encode(contract.get_counter_party_id().serialize()))
            .bind(offer_collateral.to_sat() as i64)
            .bind(accept_collateral.to_sat() as i64)
            .bind(total_collateral.to_sat() as i64)
            // need to track this
            .bind(1_i64)
            .bind(contract.get_cet_locktime() as i32)
            .bind(contract.get_refund_locktime() as i32)
            .bind(Some(contract.get_pnl().to_sat()))
            .bind(&funding_txid)
            .bind(&cet_txid)
            .bind(announcement_id)
            .bind(&oracle_pubkey)
            .execute(&mut *tx)
            .await
            .map_err(to_storage_error)?;
        }

        let existing_data =
            sqlx::query_as::<Postgres, ContractData>("SELECT * FROM contract_data WHERE id = $1")
                .bind(&contract_id)
                .fetch_optional(&mut *tx)
                .await
                .map_err(to_storage_error)?;

        // Serialize the contract data
        let serialized_contract = serialize_contract(contract)?;

        if existing_data.is_some() {
            // Update existing contract data
            sqlx::query("UPDATE contract_data SET contract_data = $2, state = $3 WHERE id = $1")
                .bind(&contract_id)
                .bind(&serialized_contract)
                .bind(prefix as i16)
                .execute(&mut *tx)
                .await
                .map_err(to_storage_error)?;
        } else {
            // Insert new contract data
            sqlx::query(
                "INSERT INTO contract_data (id, contract_data, is_compressed, state) VALUES ($1, $2, $3, $4)",
            )
            .bind(&contract_id)
            .bind(&serialized_contract)
            .bind(false) // is_compressed
            .bind(prefix as i16)
            .execute(&mut *tx)
            .await
            .map_err(to_storage_error)?;
        }

        tx.commit().await.map_err(to_storage_error)?;

        Ok(())
    }

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

    async fn upsert_channel(
        &self,
        _channel: ddk_manager::channel::Channel,
        _contract: Option<Contract>,
    ) -> Result<(), ddk_manager::error::Error> {
        unimplemented!("Channels not supported.")
    }

    async fn delete_channel(
        &self,
        _channel_id: &ddk_manager::ChannelId,
    ) -> Result<(), ddk_manager::error::Error> {
        unimplemented!("Channels not supported.")
    }

    async fn get_signed_channels(
        &self,
        _channel_state: Option<ddk_manager::channel::signed_channel::SignedChannelStateType>,
    ) -> Result<Vec<ddk_manager::channel::signed_channel::SignedChannel>, ddk_manager::error::Error>
    {
        unimplemented!("Channels not supported.")
    }

    async fn get_channel(
        &self,
        _channel_id: &ddk_manager::ChannelId,
    ) -> Result<Option<ddk_manager::channel::Channel>, ddk_manager::error::Error> {
        unimplemented!("Channels not supported.")
    }

    async fn get_offered_channels(
        &self,
    ) -> Result<Vec<ddk_manager::channel::offered_channel::OfferedChannel>, ddk_manager::error::Error>
    {
        unimplemented!("Channels not supported.")
    }

    async fn persist_chain_monitor(
        &self,
        _monitor: &ddk_manager::chain_monitor::ChainMonitor,
    ) -> Result<(), ddk_manager::error::Error> {
        unimplemented!("Chain monitor not supported.")
    }

    async fn get_chain_monitor(
        &self,
    ) -> Result<Option<ddk_manager::chain_monitor::ChainMonitor>, ddk_manager::error::Error> {
        Ok(None)
    }
}

/// Insert keychain descriptors.
#[tracing::instrument]
async fn insert_descriptor(
    tx: &mut Transaction<'_, Postgres>,
    wallet_name: &str,
    descriptor: &ExtendedDescriptor,
    keychain: KeychainKind,
) -> Result<(), SqlxError> {
    info!("insert descriptor");
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
#[tracing::instrument]
async fn insert_network(
    tx: &mut Transaction<'_, Postgres>,
    wallet_name: &str,
    network: Network,
) -> Result<(), SqlxError> {
    info!("insert network");
    sqlx::query("INSERT INTO network (wallet_name, name) VALUES ($1, $2)")
        .bind(wallet_name)
        .bind(network.to_string())
        .execute(&mut **tx)
        .await?;

    Ok(())
}

/// Update keychain last revealed
#[tracing::instrument]
async fn update_last_revealed(
    tx: &mut Transaction<'_, Postgres>,
    wallet_name: &str,
    descriptor_id: DescriptorId,
    last_revealed: u32,
) -> Result<(), SqlxError> {
    info!("update last revealed");

    sqlx::query(
        "UPDATE keychain SET last_revealed = $1 WHERE wallet_name = $2 AND descriptor_id = $3",
    )
    .bind(last_revealed as i32)
    .bind(wallet_name)
    .bind(descriptor_id.to_byte_array())
    .execute(&mut **tx)
    .await?;

    Ok(())
}

/// Select transactions, txouts, and anchors.
#[tracing::instrument]
async fn tx_graph_changeset_from_postgres(
    db_tx: &mut Transaction<'_, Postgres>,
    wallet_name: &str,
) -> Result<tx_graph::ChangeSet<ConfirmationBlockTime>, SqlxError> {
    info!("tx graph changeset from postgres");
    let mut changeset = tx_graph::ChangeSet::default();

    // Fetch transactions
    let rows = sqlx::query("SELECT txid, whole_tx, last_seen FROM tx WHERE wallet_name = $1")
        .bind(wallet_name)
        .fetch_all(&mut **db_tx)
        .await?;

    for row in rows {
        let txid: String = row.get("txid");
        let txid = Txid::from_str(&txid)?;
        let whole_tx: Option<Vec<u8>> = row.get("whole_tx");
        let last_seen: Option<i64> = row.get("last_seen");

        if let Some(tx_bytes) = whole_tx {
            if let Ok(tx) = bitcoin::Transaction::consensus_decode(&mut tx_bytes.as_slice()) {
                changeset.txs.insert(Arc::new(tx));
            }
        }
        if let Some(last_seen) = last_seen {
            changeset.last_seen.insert(txid, last_seen as u64);
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
#[tracing::instrument]
async fn tx_graph_changeset_persist_to_postgres(
    db_tx: &mut Transaction<'_, Postgres>,
    wallet_name: &str,
    changeset: &tx_graph::ChangeSet<ConfirmationBlockTime>,
) -> Result<(), SqlxError> {
    info!("tx graph changeset from postgres");
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

    for (&txid, &last_seen) in &changeset.last_seen {
        sqlx::query("UPDATE tx SET last_seen = $1 WHERE wallet_name = $2 AND txid = $3")
            .bind(last_seen as i64)
            .bind(wallet_name)
            .bind(txid.to_string())
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

/// Select blocks.
#[tracing::instrument]
async fn local_chain_changeset_from_postgres(
    db_tx: &mut Transaction<'_, Postgres>,
    wallet_name: &str,
) -> Result<local_chain::ChangeSet, SqlxError> {
    info!("local chain changeset from postgres");
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
#[tracing::instrument]
async fn local_chain_changeset_persist_to_postgres(
    db_tx: &mut Transaction<'_, Postgres>,
    wallet_name: &str,
    changeset: &local_chain::ChangeSet,
) -> Result<(), SqlxError> {
    info!("local chain changeset to postgres");
    for (&height, &hash) in &changeset.blocks {
        match hash {
            Some(hash) => {
                sqlx::query(
                    "INSERT INTO block (wallet_name, hash, height) VALUES ($1, $2, $3)
                     ON CONFLICT (wallet_name, hash) DO UPDATE SET height = $3",
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
#[tracing::instrument]
#[allow(dead_code)]
async fn easy_backup(db: Pool<Postgres>) -> Result<(), SqlxError> {
    info!("Starting easy backup");

    let statement = "SELECT * FROM keychain";

    let results = sqlx::query_as::<_, KeychainEntry>(statement)
        .fetch_all(&db)
        .await?;

    let json_array = json!(results);
    println!("{}", serde_json::to_string_pretty(&json_array)?);

    info!("Easy backup completed successfully");
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
    use crate::util::ser::deserialize_contract;
    use ddk_manager::Storage;

    async fn seed_db() -> PostgresStore {
        let store = PostgresStore::new(
            &std::env::var("DATABASE_URL").unwrap(),
            true,
            "test".to_string(),
        )
        .await
        .unwrap();

        let offered = include_bytes!("../../../../contract_binaries/Offered");
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
        let accept = include_bytes!("../../../../contract_binaries/Accepted");
        let accepted_contract = deserialize_contract(&accept.to_vec()).unwrap();
        store
            .update_contract(&accepted_contract)
            .await
            .expect("Failed to update accepted contract");
        let signed = include_bytes!("../../../../contract_binaries/Signed");
        let signed_contract = deserialize_contract(&signed.to_vec()).unwrap();
        store
            .update_contract(&signed_contract)
            .await
            .expect("Failed to update signed contract");
        let confirmed = include_bytes!("../../../../contract_binaries/Confirmed");
        let confirmed_contract = deserialize_contract(&confirmed.to_vec()).unwrap();
        store
            .update_contract(&confirmed_contract)
            .await
            .expect("Failed to update confirmed contract");
        let preclosed = include_bytes!("../../../../contract_binaries/PreClosed");
        let preclosed_contract = deserialize_contract(&preclosed.to_vec()).unwrap();
        store
            .update_contract(&preclosed_contract)
            .await
            .expect("Failed to update preclosed contract");

        let closed = include_bytes!("../../../../contract_binaries/Closed");
        let closed_contract = deserialize_contract(&closed.to_vec()).unwrap();
        store
            .update_contract(&closed_contract)
            .await
            .expect("Failed to update closed contract");

        store
    }

    #[tokio::test]
    async fn postgres() {
        let db = seed_db().await;

        let confirmed_rows = db.get_contract_metadata(None).await.unwrap();
        assert_eq!(confirmed_rows.len(), 1);
        assert_eq!(confirmed_rows[0].state, ContractPrefix::Closed as i16);
        let contracts = db.get_contracts().await.unwrap();
        assert!(contracts.len() > 0);
    }

    #[tokio::test]
    async fn get_contracts() {
        let db = PostgresStore::new(
            &std::env::var("DATABASE_URL").unwrap(),
            false,
            "test".to_string(),
        )
        .await
        .unwrap();
        let contracts = db.get_contracts().await;
        assert!(contracts.is_ok());
    }
}

use std::str::FromStr;

use super::sqlx::{ContractRowNoBytes, SqlxError};
use crate::error::WalletError;
use crate::transport::PeerInformation;
use crate::Storage;
use crate::{
    error::to_storage_error,
    storage::sqlx::ContractRow,
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
use dlc_messages::oracle_msgs::OracleAnnouncement;
use serde_json::json;
use sqlx::postgres::PgRow;
use sqlx::{FromRow, Pool, Postgres, Row, Transaction};
use std::sync::Arc;
use tracing::info;

/// Manages a pool of database connections.
#[derive(Debug)]
pub struct PostgresStore {
    pub(crate) pool: Pool<Postgres>,
    wallet_name: String,
}

impl PostgresStore {
    pub async fn new(url: &str, migrations: bool, wallet_name: String) -> Result<Self, SqlxError> {
        let pool = Pool::<Postgres>::connect(url).await?;
        if migrations {
            tracing::info!("Migrating postgres");
            sqlx::migrate!("src/storage/postgres/migrations")
                .run(&pool)
                .await?;
        }

        Ok(Self { pool, wallet_name })
    }

    pub async fn get_contract_rows(
        &self,
        states: Option<Vec<ContractPrefix>>,
    ) -> Result<Vec<ContractRowNoBytes>, SqlxError> {
        let rows = if let Some(states) = states {
            let placeholders = (1..=states.len())
                .map(|i| format!("${i}"))
                .collect::<Vec<_>>()
                .join(", ");

            let query = format!("SELECT id, state, is_offer_party, counter_party, offer_collateral, accept_collateral, total_collateral, fee_rate_per_vb, cet_locktime, refund_locktime, pnl FROM contracts WHERE state IN ({placeholders})");

            let mut query = sqlx::query_as::<_, ContractRowNoBytes>(&query);

            for state in states {
                query = query.bind(state as i16);
            }

            query.fetch_all(&self.pool).await?
        } else {
            sqlx::query_as::<Postgres, ContractRowNoBytes>(
                "SELECT id, state, is_offer_party, counter_party, offer_collateral, accept_collateral, total_collateral, fee_rate_per_vb, cet_locktime, refund_locktime, pnl FROM contracts"
            )
                .fetch_all(&self.pool)
                .await?
        };
        Ok(rows)
    }

    pub async fn get_offer_rows(&self) -> Result<Vec<ContractRowNoBytes>, SqlxError> {
        let rows = sqlx::query_as::<Postgres, ContractRowNoBytes>(
            "SELECT id, state, is_offer_party, counter_party, offer_collateral, accept_collateral, total_collateral, fee_rate_per_vb, cet_locktime, refund_locktime, pnl FROM contracts WHERE state = 1"
        )
            .fetch_all(&self.pool)
            .await?;
        Ok(rows)
    }

    #[tracing::instrument]
    pub(crate) async fn read(&self) -> Result<ChangeSet, SqlxError> {
        let mut tx = self.pool.begin().await?;
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
            .await?;

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
    ) -> Result<(), SqlxError> {
        tracing::info!("changeset from row");

        let network: String = row.get("network");
        let internal_last_revealed: Option<i32> = row.get("internal_last_revealed");
        let external_last_revealed: Option<i32> = row.get("external_last_revealed");
        let internal_desc_str: Option<String> = row.get("internal_descriptor");
        let external_desc_str: Option<String> = row.get("external_descriptor");

        changeset.network = Some(Network::from_str(&network).expect("parse Network"));

        if let Some(desc_str) = external_desc_str {
            let descriptor: Descriptor<DescriptorPublicKey> = desc_str.parse()?;
            let did = descriptor.descriptor_id();
            changeset.descriptor = Some(descriptor);
            if let Some(last_rev) = external_last_revealed {
                changeset.indexer.last_revealed.insert(did, last_rev as u32);
            }
        }

        if let Some(desc_str) = internal_desc_str {
            let descriptor: Descriptor<DescriptorPublicKey> = desc_str.parse()?;
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
    pub(crate) async fn write(&self, changeset: &ChangeSet) -> Result<(), SqlxError> {
        tracing::info!("changeset write");
        if changeset.is_empty() {
            return Ok(());
        }

        let wallet_name = &self.wallet_name;
        let mut tx = self.pool.begin().await?;

        if let Some(ref descriptor) = changeset.descriptor {
            insert_descriptor(&mut tx, wallet_name, descriptor, External).await?;
        }

        if let Some(ref change_descriptor) = changeset.change_descriptor {
            insert_descriptor(&mut tx, wallet_name, change_descriptor, Internal).await?;
        }

        if let Some(network) = changeset.network {
            insert_network(&mut tx, wallet_name, network).await?;
        }

        let last_revealed_indices = &changeset.indexer.last_revealed;
        if !last_revealed_indices.is_empty() {
            for (desc_id, index) in last_revealed_indices {
                update_last_revealed(&mut tx, wallet_name, *desc_id, *index).await?;
            }
        }

        local_chain_changeset_persist_to_postgres(&mut tx, wallet_name, &changeset.local_chain)
            .await?;
        tx_graph_changeset_persist_to_postgres(&mut tx, wallet_name, &changeset.tx_graph).await?;

        tx.commit().await?;

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

    fn list_peers(&self) -> anyhow::Result<Vec<PeerInformation>> {
        unimplemented!("Not implemented to list peers")
    }

    fn save_peer(&self, _peer: PeerInformation) -> anyhow::Result<()> {
        unimplemented!("Not implemented to save peer")
    }

    fn save_announcement(&self, _announcement: OracleAnnouncement) -> anyhow::Result<()> {
        unimplemented!("Not implemented to save announcement")
    }

    fn get_marketplace_announcements(&self) -> anyhow::Result<Vec<OracleAnnouncement>> {
        unimplemented!("Not implemented to get marketplace announcements")
    }
}

#[async_trait::async_trait]
impl ManagerStorage for PostgresStore {
    async fn get_contract(
        &self,
        id: &ddk_manager::ContractId,
    ) -> Result<Option<ddk_manager::contract::Contract>, ddk_manager::error::Error> {
        let contract =
            sqlx::query_as::<Postgres, ContractRow>("SELECT * FROM contracts WHERE id = $1")
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

    async fn get_contracts(
        &self,
    ) -> Result<Vec<ddk_manager::contract::Contract>, ddk_manager::error::Error> {
        let contracts = sqlx::query_as::<Postgres, ContractRow>("SELECT * FROM contracts")
            .fetch_all(&self.pool)
            .await
            .map_err(to_storage_error)?;

        Ok(contracts
            .into_iter()
            .map(|c| deserialize_contract(&c.contract_data).unwrap())
            .collect())
    }

    async fn create_contract(
        &self,
        contract: &OfferedContract,
    ) -> Result<(), ddk_manager::error::Error> {
        sqlx::query_as::<Postgres, ContractRow>(
            r#"
           INSERT INTO contracts (
               id, state, is_offer_party, counter_party,
               offer_collateral, accept_collateral, total_collateral, fee_rate_per_vb, 
               cet_locktime, refund_locktime, pnl, contract_data
           )
           VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12)
           RETURNING *
           "#,
        )
        .bind(hex::encode(contract.id))
        .bind(1_i16)
        .bind(contract.is_offer_party)
        .bind(hex::encode(contract.counter_party.serialize()))
        .bind(contract.offer_params.collateral as i64)
        .bind((contract.total_collateral - contract.offer_params.collateral) as i64)
        .bind(contract.total_collateral as i64)
        .bind(contract.fee_rate_per_vb as i64)
        .bind(contract.cet_locktime as i32)
        .bind(contract.refund_locktime as i32)
        .bind(None as Option<i64>)
        .bind(serialize_contract(&Contract::Offered(contract.clone()))?)
        .fetch_one(&self.pool)
        .await
        .map_err(to_storage_error)?;

        Ok(())
    }

    async fn delete_contract(
        &self,
        id: &ddk_manager::ContractId,
    ) -> Result<(), ddk_manager::error::Error> {
        let id = hex::encode(id);
        sqlx::query_as::<Postgres, ContractRow>("DELETE FROM contracts WHERE id = $1")
            .bind(id)
            .fetch_one(&self.pool)
            .await
            .map_err(to_storage_error)?;

        Ok(())
    }

    async fn update_contract(
        &self,
        contract: &ddk_manager::contract::Contract,
    ) -> Result<(), ddk_manager::error::Error> {
        tracing::info!("Updating contract. {:?}", contract.get_id());
        let prefix = ContractPrefix::get_prefix(contract);
        let serialized_contract = serialize_contract(contract)?;
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
                sqlx::query_as::<Postgres, ContractRow>("DELETE FROM contracts WHERE id = $1")
                    .bind(temp_id)
                    .fetch_all(&mut *tx)
                    .await
                    .map_err(to_storage_error)?;
            }
            _ => {}
        }

        // Step 2: Upsert the contract by id
        sqlx::query_as::<Postgres, ContractRow>(
            r#"
            INSERT INTO contracts (
               id, state, is_offer_party, counter_party,
               offer_collateral, accept_collateral, total_collateral, fee_rate_per_vb, 
               cet_locktime, refund_locktime, pnl, contract_data
           )
           VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12)
            ON CONFLICT (id)
            DO UPDATE SET
                id = EXCLUDED.id,
                state = EXCLUDED.state,
                contract_data = EXCLUDED.contract_data,
                pnl = EXCLUDED.pnl
            "#,
        )
        .bind(contract_id)
        .bind(prefix as i16)
        .bind(false)
        .bind(hex::encode(contract.get_counter_party_id().serialize()))
        .bind(offer_collateral as i64)
        .bind(accept_collateral as i64)
        .bind(total_collateral as i64)
        .bind(1_i64)
        .bind(contract.get_cet_locktime() as i32)
        .bind(contract.get_refund_locktime() as i32)
        .bind(Some(contract.get_pnl()))
        .bind(serialized_contract)
        .fetch_all(&mut *tx)
        .await
        .map_err(to_storage_error)?;

        // Commit the transaction
        tx.commit().await.map_err(to_storage_error)?;

        Ok(())
    }

    async fn get_signed_contracts(&self) -> Result<Vec<SignedContract>, ddk_manager::error::Error> {
        let contracts =
            sqlx::query_as::<Postgres, ContractRow>("SELECT * FROM contracts WHERE state = 3")
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
        let contracts = sqlx::query_as::<Postgres, ContractRow>(
            "SELECT * FROM contracts WHERE state = 1 AND is_offer_party = false",
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
            sqlx::query_as::<Postgres, ContractRow>("SELECT * FROM contracts WHERE state = 4")
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
            sqlx::query_as::<Postgres, ContractRow>("SELECT * FROM contracts WHERE state = 5")
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
        _contract: Option<ddk_manager::contract::Contract>,
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
struct KeychainEntry {
    wallet_name: String,
    keychainkind: String,
    descriptor: String,
    descriptor_id: Vec<u8>,
    last_revealed: i32,
}

#[cfg(test)]
mod tests {
    use ddk_manager::contract::{
        accepted_contract::AcceptedContract, offered_contract::OfferedContract, ser::Serializable,
    };

    use super::*;

    fn deserialize_object<T>(serialized: &[u8]) -> T
    where
        T: Serializable,
    {
        let mut cursor = ::lightning::io::Cursor::new(&serialized);
        T::deserialize(&mut cursor).unwrap()
    }

    async fn seed_db() -> PostgresStore {
        let store = PostgresStore::new(
            &std::env::var("DATABASE_URL").unwrap(),
            true,
            "test".to_string(),
        )
        .await
        .unwrap();

        let accept = include_bytes!("../../../tests/data/dlc_storage/Accepted");
        let accepted_contract = deserialize_object::<AcceptedContract>(&accept.to_vec());
        store
            .update_contract(&Contract::Accepted(accepted_contract))
            .await
            .expect("Failed to update accepted contract");
        let signed = include_bytes!("../../../tests/data/dlc_storage/Signed");
        let signed_contract = deserialize_object::<SignedContract>(&signed.to_vec());
        store
            .update_contract(&Contract::Signed(signed_contract))
            .await
            .expect("Failed to update signed contract");
        let confirmed = include_bytes!("../../../tests/data/dlc_storage/Confirmed");
        let confirmed_contract = deserialize_object::<SignedContract>(&confirmed.to_vec());
        store
            .update_contract(&Contract::Confirmed(confirmed_contract))
            .await
            .expect("Failed to update confirmed contract");
        let preclosed = include_bytes!("../../../tests/data/dlc_storage/PreClosed");
        let preclosed_contract = deserialize_object::<PreClosedContract>(&preclosed.to_vec());
        store
            .update_contract(&Contract::PreClosed(preclosed_contract))
            .await
            .expect("Failed to update preclosed contract");
        let offered = include_bytes!("../../../tests/data/dlc_storage/Offered");
        let offered_contract = deserialize_object::<OfferedContract>(&offered.to_vec());
        store
            .update_contract(&Contract::Offered(offered_contract))
            .await
            .expect("Failed to update offered contract");

        store
    }

    #[tokio::test]
    async fn postgres() {
        let db = seed_db().await;

        let offer_rows = db.get_offer_rows().await.unwrap();
        assert_eq!(offer_rows.len(), 1);
        assert_eq!(offer_rows[0].state, ContractPrefix::Offered as i16);

        let signed_prefix: ContractPrefix = "signed".to_string().into();
        let confirmed_prefix: ContractPrefix = "confirmed".to_string().into();
        let confirmed_rows = db
            .get_contract_rows(Some(vec![signed_prefix, confirmed_prefix]))
            .await
            .unwrap();
        assert_eq!(confirmed_rows.len(), 2);
        assert_eq!(confirmed_rows[0].state, ContractPrefix::Signed as i16);
        assert_eq!(confirmed_rows[1].state, ContractPrefix::Confirmed as i16);
    }
}

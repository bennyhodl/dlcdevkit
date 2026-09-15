pub mod contract_row;
pub mod legacy;

use super::sqlx::{ContractMetadata, SqlxError};
use crate::error::{StorageError, WalletError};
use crate::logger::Logger;
use crate::logger::{log_debug, log_info, WriteLog};
use crate::Storage;
use crate::{error::to_storage_error, util::ser::ContractPrefix};
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
use contract_row::ContractRow;
use ddk_manager::{
    contract::{
        offered_contract::OfferedContract, signed_contract::SignedContract, Contract,
        PreClosedContract,
    },
    Storage as ManagerStorage,
};
pub use legacy::LegacyMigrationReport;
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
fn wrong_state_error(state: &str) -> ddk_manager::error::Error {
    ddk_manager::error::Error::StorageError(format!("contract is not in the {state} state"))
}

fn max_connections_from_env() -> u32 {
    std::env::var("DATABASE_MAX_CONNECTIONS")
        .ok()
        .and_then(|val| val.parse::<u32>().ok())
        .filter(|n| *n > 0)
        .unwrap_or(DEFAULT_MAX_CONNECTIONS)
}

/// The embedded schema migrations for the Postgres storage backend.
///
/// The migration files are compiled into the crate, so consumers can apply or
/// revert the schema against any database without access to the source tree:
///
/// ```ignore
/// ddk::storage::postgres::MIGRATOR.run(&pool).await?;          // apply up
/// ddk::storage::postgres::MIGRATOR.undo(&pool, version).await?; // revert down to `version`
/// ```
///
/// [`PostgresStore::new`] runs this same migrator when `migrations` is true.
pub static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("src/storage/postgres/migrations");

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
        let store = Self {
            pool,
            logger,
            wallet_name,
        };

        if migrations {
            store.run_migrations().await?;
        }
        // Without migrations the new table may not exist yet, and every read
        // will say so; the count is not the place to fail.
        if let Err(e) = store.warn_if_legacy_contracts_remain().await {
            log_debug!(
                store.logger,
                "Could not count contracts in the legacy blob layout. error={}",
                e
            );
        }

        Ok(store)
    }

    /// Applies the schema migrations, then moves every contract still in the
    /// legacy blob layout to the columnar layout. `new` runs this when
    /// `migrations` is on.
    pub async fn run_migrations(&self) -> Result<LegacyMigrationReport, StorageError> {
        log_info!(self.logger, "Migrating postgres");
        MIGRATOR
            .run(&self.pool)
            .await
            .map_err(|e| StorageError::Sqlx(e.into()))?;
        self.migrate_legacy_contracts()
            .await
            .map_err(|e| StorageError::Init(e.to_string()))
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

            let query =
                format!("SELECT * FROM ({CONTRACT_METADATA}) c WHERE c.state IN ({placeholders})");

            let mut query = sqlx::query_as::<_, ContractMetadata>(&query);

            for state in states {
                query = query.bind(state as i16);
            }

            query
                .fetch_all(&self.pool)
                .await
                .map_err(|e| StorageError::Sqlx(e.into()))?
        } else {
            sqlx::query_as::<Postgres, ContractMetadata>(CONTRACT_METADATA)
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
        let row = sqlx::query_as::<Postgres, ContractMetadata>(&format!(
            "SELECT * FROM ({CONTRACT_METADATA}) c WHERE c.id = $1"
        ))
        .bind(id)
        .fetch_one(&self.pool)
        .await
        .map_err(|e| StorageError::Sqlx(e.into()))?;
        Ok(row)
    }

    pub async fn get_offer_metadata(&self) -> Result<Vec<ContractMetadata>, StorageError> {
        let rows = sqlx::query_as::<Postgres, ContractMetadata>(&format!(
            "SELECT * FROM ({CONTRACT_METADATA}) c WHERE c.state = 1"
        ))
        .fetch_all(&self.pool)
        .await
        .map_err(|e| StorageError::Sqlx(e.into()))?;
        Ok(rows)
    }

    /// The contracts in `state`, read through the one decoder, plus any
    /// still in the legacy blob layout.
    async fn contracts_in_state(
        &self,
        state: ContractPrefix,
    ) -> Result<Vec<Contract>, ddk_manager::error::Error> {
        let state = state as i16;
        let rows =
            sqlx::query_as::<Postgres, ContractRow>("SELECT * FROM dlc_contracts WHERE state = $1")
                .bind(state)
                .fetch_all(&self.pool)
                .await
                .map_err(to_storage_error)?;
        let mut contracts = rows
            .into_iter()
            .map(ContractRow::into_contract)
            .collect::<Result<Vec<_>, _>>()?;
        contracts.extend(self.legacy_contracts(Some(state)).await?);
        Ok(contracts)
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
        changeset.locked_outpoints = locked_outpoints_from_postgres(tx, wallet_name).await?;
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

        locked_outpoints_persist_to_postgres(&mut tx, wallet_name, &changeset.locked_outpoints)
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

    async fn initialize_contract_tracker(
        &self,
    ) -> Result<crate::wallet::contract_tracker::ChangeSet, WalletError> {
        let row = sqlx::query("SELECT changeset FROM contract_tracker WHERE wallet_name = $1")
            .bind(&self.wallet_name)
            .fetch_optional(&self.pool)
            .await
            .map_err(|e| WalletError::StorageError(e.to_string()))?;

        match row {
            Some(row) => {
                let changeset: serde_json::Value = row.get("changeset");
                Ok(serde_json::from_value(changeset)?)
            }
            None => Ok(crate::wallet::contract_tracker::ChangeSet::default()),
        }
    }

    async fn persist_contract_tracker(
        &self,
        changeset: &crate::wallet::contract_tracker::ChangeSet,
    ) -> Result<(), WalletError> {
        let mut tx = self
            .pool
            .begin()
            .await
            .map_err(|e| WalletError::StorageError(e.to_string()))?;

        // Merge with the stored changeset app-side; the changeset is
        // monotone under merge.
        let stored =
            sqlx::query("SELECT changeset FROM contract_tracker WHERE wallet_name = $1 FOR UPDATE")
                .bind(&self.wallet_name)
                .fetch_optional(&mut *tx)
                .await
                .map_err(|e| WalletError::StorageError(e.to_string()))?;

        let mut merged = match stored {
            Some(row) => {
                let value: serde_json::Value = row.get("changeset");
                serde_json::from_value::<crate::wallet::contract_tracker::ChangeSet>(value)?
            }
            None => crate::wallet::contract_tracker::ChangeSet::default(),
        };
        merged.merge(changeset.clone());

        sqlx::query(
            "INSERT INTO contract_tracker (wallet_name, changeset) VALUES ($1, $2)
             ON CONFLICT (wallet_name) DO UPDATE SET changeset = $2",
        )
        .bind(&self.wallet_name)
        .bind(serde_json::to_value(&merged)?)
        .execute(&mut *tx)
        .await
        .map_err(|e| WalletError::StorageError(e.to_string()))?;

        tx.commit()
            .await
            .map_err(|e| WalletError::StorageError(e.to_string()))?;
        Ok(())
    }

    async fn load_labels(&self) -> Result<bip329::Labels, WalletError> {
        let rows = sqlx::query("SELECT label FROM wallet_labels WHERE wallet_name = $1")
            .bind(&self.wallet_name)
            .fetch_all(&self.pool)
            .await
            .map_err(|e| WalletError::StorageError(e.to_string()))?;

        let labels = rows
            .into_iter()
            .map(|row| {
                let label: serde_json::Value = row.get("label");
                Ok(serde_json::from_value::<bip329::Label>(label)?)
            })
            .collect::<Result<Vec<_>, WalletError>>()?;
        Ok(bip329::Labels::new(labels))
    }

    async fn persist_label(&self, label: &bip329::Label) -> Result<(), WalletError> {
        sqlx::query(
            "INSERT INTO wallet_labels (wallet_name, label_key, label) VALUES ($1, $2, $3)
             ON CONFLICT (wallet_name, label_key) DO UPDATE SET label = $3",
        )
        .bind(&self.wallet_name)
        .bind(crate::storage::label_key(&label.ref_()))
        .bind(serde_json::to_value(label)?)
        .execute(&self.pool)
        .await
        .map_err(|e| WalletError::StorageError(e.to_string()))?;
        Ok(())
    }

    async fn delete_label(&self, label_ref: &bip329::LabelRef) -> Result<(), WalletError> {
        sqlx::query("DELETE FROM wallet_labels WHERE wallet_name = $1 AND label_key = $2")
            .bind(&self.wallet_name)
            .bind(crate::storage::label_key(label_ref))
            .execute(&self.pool)
            .await
            .map_err(|e| WalletError::StorageError(e.to_string()))?;
        Ok(())
    }
}

#[async_trait::async_trait]
impl ManagerStorage for PostgresStore {
    #[tracing::instrument(skip(self))]
    async fn get_contract(
        &self,
        id: &ddk_manager::ContractId,
    ) -> Result<Option<Contract>, ddk_manager::error::Error> {
        let id = hex::encode(id);
        let row =
            sqlx::query_as::<Postgres, ContractRow>("SELECT * FROM dlc_contracts WHERE id = $1")
                .bind(&id)
                .fetch_optional(&self.pool)
                .await
                .map_err(to_storage_error)?;

        match row {
            Some(row) => Ok(Some(row.into_contract()?)),
            None => self.legacy_contract(&id).await,
        }
    }

    #[tracing::instrument(skip(self))]
    async fn get_contracts(&self) -> Result<Vec<Contract>, ddk_manager::error::Error> {
        let rows = sqlx::query_as::<Postgres, ContractRow>("SELECT * FROM dlc_contracts")
            .fetch_all(&self.pool)
            .await
            .map_err(to_storage_error)?;

        let mut contracts = rows
            .into_iter()
            .map(ContractRow::into_contract)
            .collect::<Result<Vec<_>, _>>()?;
        contracts.extend(self.legacy_contracts(None).await?);
        Ok(contracts)
    }

    async fn create_contract(
        &self,
        contract: &OfferedContract,
    ) -> Result<(), ddk_manager::error::Error> {
        let row = ContractRow::from_contract(&Contract::Offered(contract.clone()))?;
        let mut tx = self.pool.begin().await.map_err(to_storage_error)?;
        upsert_contract_row(&mut tx, &row).await?;
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
        delete_contract_rows(&mut tx, &id).await?;
        tx.commit().await.map_err(to_storage_error)?;

        Ok(())
    }

    async fn update_contract(&self, contract: &Contract) -> Result<(), ddk_manager::error::Error> {
        log_info!(
            self.logger,
            "Updating contract. id={}",
            hex::encode(contract.get_id())
        );
        let row = ContractRow::from_contract(contract)?;

        let mut tx = self.pool.begin().await.map_err(to_storage_error)?;

        // The offered row is keyed by the temporary id. Once the contract has
        // its real id, that row goes.
        match contract {
            Contract::Accepted(_) | Contract::Signed(_) => {
                log_info!(
                    self.logger,
                    "Deleting contract by temp_id. tmp_id={}",
                    row.temporary_id
                );
                delete_contract_rows(&mut tx, &row.temporary_id).await?;
            }
            _ => {}
        }

        upsert_contract_row(&mut tx, &row).await?;
        // A contract read from the legacy blob layout moves over on its
        // first update.
        legacy::delete_legacy_rows(&mut tx, &row.id).await?;

        tx.commit().await.map_err(to_storage_error)?;

        Ok(())
    }

    #[tracing::instrument(skip(self))]
    async fn get_signed_contracts(&self) -> Result<Vec<SignedContract>, ddk_manager::error::Error> {
        self.contracts_in_state(ContractPrefix::Signed)
            .await?
            .into_iter()
            .map(|c| match c {
                Contract::Signed(s) => Ok(s),
                _ => Err(wrong_state_error("signed")),
            })
            .collect()
    }

    #[tracing::instrument(skip(self))]
    async fn get_contract_offers(&self) -> Result<Vec<OfferedContract>, ddk_manager::error::Error> {
        self.contracts_in_state(ContractPrefix::Offered)
            .await?
            .into_iter()
            .filter(|c| !c.is_offer_party())
            .map(|c| match c {
                Contract::Offered(o) => Ok(o),
                _ => Err(wrong_state_error("offered")),
            })
            .collect()
    }

    #[tracing::instrument(skip(self))]
    async fn get_confirmed_contracts(
        &self,
    ) -> Result<Vec<SignedContract>, ddk_manager::error::Error> {
        self.contracts_in_state(ContractPrefix::Confirmed)
            .await?
            .into_iter()
            .map(|c| match c {
                Contract::Confirmed(s) => Ok(s),
                _ => Err(wrong_state_error("confirmed")),
            })
            .collect()
    }

    #[tracing::instrument(skip(self))]
    async fn get_preclosed_contracts(
        &self,
    ) -> Result<Vec<PreClosedContract>, ddk_manager::error::Error> {
        self.contracts_in_state(ContractPrefix::PreClosed)
            .await?
            .into_iter()
            .map(|c| match c {
                Contract::PreClosed(p) => Ok(p),
                _ => Err(wrong_state_error("pre-closed")),
            })
            .collect()
    }
}

/// The metadata columns, from `dlc_contracts` and from the legacy
/// `contract_metadata` rows that have not been migrated yet.
const CONTRACT_METADATA: &str = "SELECT id, state, is_offer_party, counter_party,
        offer_collateral, accept_collateral, total_collateral, fee_rate_per_vb,
        cet_locktime, refund_locktime, pnl, funding_txid, cet_txid, announcement_id, oracle_pubkey
    FROM dlc_contracts
    UNION ALL
    SELECT id, state, is_offer_party, counter_party,
        offer_collateral, accept_collateral, total_collateral, fee_rate_per_vb,
        cet_locktime, refund_locktime, pnl, funding_txid, cet_txid, announcement_id, oracle_pubkey
    FROM contract_metadata m
    WHERE NOT EXISTS (SELECT 1 FROM dlc_contracts d WHERE d.id = m.id)";

/// Writes a contract row, replacing the row with the same id.
pub(super) async fn upsert_contract_row(
    tx: &mut Transaction<'_, Postgres>,
    row: &ContractRow,
) -> Result<(), ddk_manager::error::Error> {
    sqlx::query(
        r#"
        INSERT INTO dlc_contracts (
            id, format_version, state, temporary_id, is_offer_party, counter_party,
            keys_id, contract_flags, chain_hash,
            offer_collateral, accept_collateral, total_collateral, fee_rate_per_vb,
            cet_locktime, refund_locktime, announcement_id, oracle_pubkey,
            funding_txid, cet_txid, pnl,
            offer_message, accept_message, sign_message,
            offer_params, accept_params, adaptor_infos, dlc_transactions,
            channel_id, attestations, signed_cet, error_message
        )
        VALUES (
            $1, $2, $3, $4, $5, $6,
            $7, $8, $9,
            $10, $11, $12, $13,
            $14, $15, $16, $17,
            $18, $19, $20,
            $21, $22, $23,
            $24, $25, $26, $27,
            $28, $29, $30, $31
        )
        ON CONFLICT (id) DO UPDATE SET
            format_version = EXCLUDED.format_version,
            state = EXCLUDED.state,
            temporary_id = EXCLUDED.temporary_id,
            is_offer_party = EXCLUDED.is_offer_party,
            counter_party = EXCLUDED.counter_party,
            keys_id = EXCLUDED.keys_id,
            contract_flags = EXCLUDED.contract_flags,
            chain_hash = EXCLUDED.chain_hash,
            offer_collateral = EXCLUDED.offer_collateral,
            accept_collateral = EXCLUDED.accept_collateral,
            total_collateral = EXCLUDED.total_collateral,
            fee_rate_per_vb = EXCLUDED.fee_rate_per_vb,
            cet_locktime = EXCLUDED.cet_locktime,
            refund_locktime = EXCLUDED.refund_locktime,
            announcement_id = EXCLUDED.announcement_id,
            oracle_pubkey = EXCLUDED.oracle_pubkey,
            funding_txid = EXCLUDED.funding_txid,
            cet_txid = EXCLUDED.cet_txid,
            pnl = EXCLUDED.pnl,
            offer_message = EXCLUDED.offer_message,
            accept_message = EXCLUDED.accept_message,
            sign_message = EXCLUDED.sign_message,
            offer_params = EXCLUDED.offer_params,
            accept_params = EXCLUDED.accept_params,
            adaptor_infos = EXCLUDED.adaptor_infos,
            dlc_transactions = EXCLUDED.dlc_transactions,
            channel_id = EXCLUDED.channel_id,
            attestations = EXCLUDED.attestations,
            signed_cet = EXCLUDED.signed_cet,
            error_message = EXCLUDED.error_message,
            updated_at = now()
        "#,
    )
    .bind(&row.id)
    .bind(row.format_version)
    .bind(row.state)
    .bind(&row.temporary_id)
    .bind(row.is_offer_party)
    .bind(&row.counter_party)
    .bind(&row.keys_id)
    .bind(row.contract_flags)
    .bind(&row.chain_hash)
    .bind(row.offer_collateral)
    .bind(row.accept_collateral)
    .bind(row.total_collateral)
    .bind(row.fee_rate_per_vb)
    .bind(row.cet_locktime)
    .bind(row.refund_locktime)
    .bind(&row.announcement_id)
    .bind(&row.oracle_pubkey)
    .bind(&row.funding_txid)
    .bind(&row.cet_txid)
    .bind(row.pnl)
    .bind(&row.offer_message)
    .bind(&row.accept_message)
    .bind(&row.sign_message)
    .bind(&row.offer_params)
    .bind(&row.accept_params)
    .bind(&row.adaptor_infos)
    .bind(&row.dlc_transactions)
    .bind(&row.channel_id)
    .bind(&row.attestations)
    .bind(&row.signed_cet)
    .bind(&row.error_message)
    .execute(&mut **tx)
    .await
    .map_err(to_storage_error)?;
    Ok(())
}

/// Deletes a contract from `dlc_contracts` and from the legacy tables.
async fn delete_contract_rows(
    tx: &mut Transaction<'_, Postgres>,
    id: &str,
) -> Result<(), ddk_manager::error::Error> {
    sqlx::query("DELETE FROM dlc_contracts WHERE id = $1")
        .bind(id)
        .execute(&mut **tx)
        .await
        .map_err(to_storage_error)?;
    legacy::delete_legacy_rows(tx, id).await
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

    // A wallet's descriptors never change once created; re-staging one must
    // not poison the persist transaction with a unique violation.
    sqlx::query(
        "INSERT INTO keychain (wallet_name, keychainkind, descriptor, descriptor_id) VALUES ($1, $2, $3, $4)
         ON CONFLICT (wallet_name, keychainkind) DO NOTHING",
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
    sqlx::query(
        "INSERT INTO network (wallet_name, name) VALUES ($1, $2)
         ON CONFLICT (wallet_name) DO NOTHING",
    )
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

/// Select the wallet's locked outpoints.
#[tracing::instrument(skip(db_tx))]
async fn locked_outpoints_from_postgres(
    db_tx: &mut Transaction<'_, Postgres>,
    wallet_name: &str,
) -> Result<bdk_wallet::locked_outpoints::ChangeSet, SqlxError> {
    let rows =
        sqlx::query("SELECT txid, vout, is_locked FROM locked_outpoints WHERE wallet_name = $1")
            .bind(wallet_name)
            .fetch_all(&mut **db_tx)
            .await?;

    let mut outpoints = BTreeMap::new();
    for row in rows {
        let txid: String = row.get("txid");
        let vout: i32 = row.get("vout");
        let is_locked: bool = row.get("is_locked");
        let txid =
            Txid::from_str(&txid).map_err(|_| SqlxError::Custom("parse locked txid".into()))?;
        outpoints.insert(
            OutPoint {
                txid,
                vout: vout as u32,
            },
            is_locked,
        );
    }

    Ok(bdk_wallet::locked_outpoints::ChangeSet { outpoints })
}

/// Upsert the wallet's locked outpoints. A `false` overwrites an earlier
/// `true`, matching the merge semantics of the BDK changeset.
#[tracing::instrument(skip_all)]
async fn locked_outpoints_persist_to_postgres(
    db_tx: &mut Transaction<'_, Postgres>,
    wallet_name: &str,
    changeset: &bdk_wallet::locked_outpoints::ChangeSet,
) -> Result<(), SqlxError> {
    for (outpoint, is_locked) in &changeset.outpoints {
        sqlx::query(
            "INSERT INTO locked_outpoints (wallet_name, txid, vout, is_locked)
             VALUES ($1, $2, $3, $4)
             ON CONFLICT (wallet_name, txid, vout) DO UPDATE SET is_locked = $4",
        )
        .bind(wallet_name)
        .bind(outpoint.txid.to_string())
        .bind(outpoint.vout as i32)
        .bind(is_locked)
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

    let rows = sqlx::query(
        "SELECT descriptor_id, spk_index, script FROM spk_cache WHERE wallet_name = $1",
    )
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
    last_revealed: Option<i32>,
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
    async fn labels_round_trip() {
        use crate::Storage as DdkStorage;

        let (_server, db) = seed_db().await;
        let txid = dummy_tx().compute_txid();

        let label = bip329::Label::Transaction(bip329::TransactionRecord {
            ref_: txid,
            label: Some("DLC funding".to_string()),
            origin: None,
        });
        DdkStorage::persist_label(&db, &label).await.unwrap();

        let output_label = bip329::Label::Output(bip329::OutputRecord {
            ref_: OutPoint { txid, vout: 0 },
            label: Some("collateral".to_string()),
            spendable: Some(false),
        });
        DdkStorage::persist_label(&db, &output_label).await.unwrap();

        let labels = DdkStorage::load_labels(&db).await.unwrap();
        assert_eq!(labels.iter().count(), 2);

        // Replacing by reference and deleting.
        let renamed = bip329::Label::Transaction(bip329::TransactionRecord {
            ref_: txid,
            label: Some("renamed".to_string()),
            origin: None,
        });
        DdkStorage::persist_label(&db, &renamed).await.unwrap();
        DdkStorage::delete_label(&db, &output_label.ref_())
            .await
            .unwrap();

        let labels = DdkStorage::load_labels(&db).await.unwrap();
        assert_eq!(labels.iter().count(), 1);
        assert!(labels.iter().any(|label| matches!(
            label,
            bip329::Label::Transaction(record) if record.label.as_deref() == Some("renamed")
        )));
    }

    #[tokio::test]
    async fn contract_tracker_round_trips() {
        use crate::wallet::contract_tracker;
        use crate::Storage as DdkStorage;

        let (_server, db) = seed_db().await;

        let spk = ScriptBuf::new_p2wsh(&bitcoin::WScriptHash::from_byte_array([0xCD; 32]));
        let mut changeset = contract_tracker::ChangeSet::default();
        changeset.spks.insert([0x5A; 32], spk.clone());
        changeset
            .tx_graph
            .last_seen
            .insert(dummy_tx().compute_txid(), 100);
        DdkStorage::persist_contract_tracker(&db, &changeset)
            .await
            .unwrap();
        let read = DdkStorage::initialize_contract_tracker(&db).await.unwrap();
        assert_eq!(read, changeset);

        // A second persist merges instead of overwriting.
        let mut more = contract_tracker::ChangeSet::default();
        more.spks.insert([0x5B; 32], spk);
        DdkStorage::persist_contract_tracker(&db, &more)
            .await
            .unwrap();
        let read = DdkStorage::initialize_contract_tracker(&db).await.unwrap();
        assert_eq!(read.spks.len(), 2);
        assert_eq!(read.tx_graph.last_seen.len(), 1);
    }

    #[tokio::test]
    async fn locked_outpoints_round_trip() {
        let (_server, db) = seed_db().await;

        let outpoint = OutPoint {
            txid: dummy_tx().compute_txid(),
            vout: 0,
        };

        let mut lock = ChangeSet::default();
        lock.network = Some(Network::Regtest);
        lock.locked_outpoints.outpoints.insert(outpoint, true);
        db.write(&lock).await.unwrap();
        let read = db.read().await.unwrap();
        assert_eq!(read.locked_outpoints.outpoints.get(&outpoint), Some(&true));

        // An unlock overwrites the lock.
        let mut unlock = ChangeSet::default();
        unlock.locked_outpoints.outpoints.insert(outpoint, false);
        db.write(&unlock).await.unwrap();
        let read = db.read().await.unwrap();
        assert_eq!(read.locked_outpoints.outpoints.get(&outpoint), Some(&false));
    }

    #[tokio::test]
    async fn descriptor_and_network_writes_are_idempotent() {
        let (_server, db) = seed_db().await;

        let descriptor: ExtendedDescriptor = "wpkh([73c5da0a/84'/1'/0']tpubDC8msFGeGuwnKG9Upg7DM2b4DaRqg3CUZa5g8v2SRQ6K4NSkxUgd7HsL2XVWbVm39yBA4LAxysQAm397zwQSQoQgewGiYZqrA9DsP4zbQ1M/0/*)"
            .parse()
            .unwrap();
        let did = descriptor.descriptor_id();

        let mut changeset = ChangeSet::default();
        changeset.network = Some(Network::Regtest);
        changeset.descriptor = Some(descriptor.clone());
        changeset.indexer.last_revealed.insert(did, 4);
        db.write(&changeset).await.unwrap();

        // Re-staging the descriptor and network (wallet re-create path) must
        // not violate unique constraints or reset last_revealed.
        db.write(&changeset).await.unwrap();

        // A keychain row written without a revealed index must read back as
        // "nothing revealed", not index 0.
        let mut fresh = ChangeSet::default();
        fresh.change_descriptor = Some(
            "wpkh([73c5da0a/84'/1'/0']tpubDC8msFGeGuwnKG9Upg7DM2b4DaRqg3CUZa5g8v2SRQ6K4NSkxUgd7HsL2XVWbVm39yBA4LAxysQAm397zwQSQoQgewGiYZqrA9DsP4zbQ1M/1/*)"
                .parse()
                .unwrap(),
        );
        db.write(&fresh).await.unwrap();
        let read = db.read().await.unwrap();
        let fresh_did = fresh.change_descriptor.as_ref().unwrap().descriptor_id();
        assert!(read.indexer.last_revealed.get(&fresh_did).is_none());

        let read = db.read().await.unwrap();
        assert_eq!(read.network, Some(Network::Regtest));
        assert_eq!(read.descriptor, Some(descriptor));
        assert_eq!(read.indexer.last_revealed.get(&did), Some(&4));
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
        assert_eq!(
            metadata[0].is_offer_party,
            accepted_contract.is_offer_party()
        );
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

    /// Writes a contract the way releases up to 2.0 did: a blob in
    /// contract_data and a metadata row in contract_metadata.
    async fn seed_legacy(pool: &Pool<Postgres>, contract: &Contract) {
        let id = hex::encode(contract.get_id());
        let state = ContractPrefix::get_prefix(contract) as i16;
        let (offer, accept, total) = contract.get_collateral();
        sqlx::query(
            "INSERT INTO contract_metadata (
                id, state, is_offer_party, counter_party, offer_collateral, accept_collateral,
                total_collateral, fee_rate_per_vb, cet_locktime, refund_locktime, pnl
            ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11)",
        )
        .bind(&id)
        .bind(state)
        .bind(contract.is_offer_party())
        .bind(hex::encode(contract.get_counter_party_id().serialize()))
        .bind(offer.to_sat() as i64)
        .bind(accept.to_sat() as i64)
        .bind(total.to_sat() as i64)
        .bind(contract.get_fee_rate_per_vb() as i64)
        .bind(contract.get_cet_locktime() as i32)
        .bind(contract.get_refund_locktime() as i32)
        .bind(Some(contract.get_pnl().to_sat()))
        .execute(pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO contract_data (id, state, contract_data, is_compressed)
             VALUES ($1, $2, $3, false)",
        )
        .bind(&id)
        .bind(state)
        .bind(crate::util::ser::serialize_contract(contract).unwrap())
        .execute(pool)
        .await
        .unwrap();
    }

    fn fixture(name: &str) -> Contract {
        let path = format!(
            "{}/../testconfig/contract_binaries/{name}",
            env!("CARGO_MANIFEST_DIR")
        );
        deserialize_contract(&std::fs::read(path).unwrap()).unwrap()
    }

    async fn open(server: &TestPostgres, migrations: bool) -> PostgresStore {
        PostgresStore::new(
            server.url(),
            migrations,
            Arc::new(Logger::console(
                "console_logger".to_string(),
                LogLevel::Info,
            )),
            "test".to_string(),
        )
        .await
        .unwrap()
    }

    fn bytes(contract: &Contract) -> Vec<u8> {
        crate::util::ser::serialize_contract(contract).unwrap()
    }

    /// A database written by 2.0 is moved to the columnar layout when the
    /// store opens with migrations on, and every contract survives.
    #[tokio::test]
    async fn legacy_contracts_migrate_at_startup() {
        let server = TestPostgres::start("ddk").await;
        let schema = open(&server, true).await;
        let offered = fixture("Offered");
        let closed = fixture("Closed");
        seed_legacy(&schema.pool, &offered).await;
        seed_legacy(&schema.pool, &closed).await;
        assert_eq!(schema.count_legacy_contracts().await.unwrap(), 2);
        drop(schema);

        let db = open(&server, true).await;
        assert_eq!(db.count_legacy_contracts().await.unwrap(), 0);

        let (rows,): (i64,) =
            sqlx::query_as("SELECT COUNT(*) FROM dlc_contracts WHERE format_version = 2")
                .fetch_one(&db.pool)
                .await
                .unwrap();
        assert_eq!(rows, 2);
        let (legacy,): (i64,) = sqlx::query_as("SELECT COUNT(*) FROM contract_data")
            .fetch_one(&db.pool)
            .await
            .unwrap();
        assert_eq!(legacy, 0);

        let read = db.get_contract(&offered.get_id()).await.unwrap().unwrap();
        assert_eq!(bytes(&read), bytes(&offered));
        let read = db.get_contract(&closed.get_id()).await.unwrap().unwrap();
        assert_eq!(bytes(&read), bytes(&closed));
        assert_eq!(db.get_contract_metadata(None).await.unwrap().len(), 2);
    }

    /// With migrations off, a legacy contract still loads through every read
    /// path and moves to the columnar layout on its first update.
    #[tokio::test]
    async fn legacy_contract_loads_and_moves_on_update() {
        let server = TestPostgres::start("ddk").await;
        let schema = open(&server, true).await;
        let signed = fixture("Signed");
        seed_legacy(&schema.pool, &signed).await;
        drop(schema);

        let db = open(&server, false).await;
        assert_eq!(db.count_legacy_contracts().await.unwrap(), 1);
        let read = db.get_contract(&signed.get_id()).await.unwrap().unwrap();
        assert_eq!(bytes(&read), bytes(&signed));
        assert_eq!(db.get_contracts().await.unwrap().len(), 1);
        assert_eq!(db.get_signed_contracts().await.unwrap().len(), 1);
        assert_eq!(db.get_contract_metadata(None).await.unwrap().len(), 1);

        db.update_contract(&signed).await.unwrap();
        assert_eq!(db.count_legacy_contracts().await.unwrap(), 0);
        assert_eq!(db.get_signed_contracts().await.unwrap().len(), 1);
        assert_eq!(db.get_contract_metadata(None).await.unwrap().len(), 1);
        let read = db.get_contract(&signed.get_id()).await.unwrap().unwrap();
        assert_eq!(bytes(&read), bytes(&signed));

        let report = db.migrate_legacy_contracts().await.unwrap();
        assert_eq!(report, LegacyMigrationReport::default());
    }

    /// A blob that cannot be decoded stays where it is, is named in the
    /// report, and does not stop the store from opening or the other
    /// contracts from moving.
    #[tokio::test]
    async fn migration_reports_the_rows_it_cannot_move() {
        let server = TestPostgres::start("ddk").await;
        let schema = open(&server, true).await;
        seed_legacy(&schema.pool, &fixture("Confirmed")).await;
        sqlx::query(
            "INSERT INTO contract_metadata (
                id, state, is_offer_party, counter_party, offer_collateral, accept_collateral,
                total_collateral, fee_rate_per_vb, cet_locktime, refund_locktime
            ) VALUES ('bad', 3, true, 'aa', 0, 0, 0, 0, 0, 0)",
        )
        .execute(&schema.pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO contract_data (id, state, contract_data, is_compressed)
             VALUES ('bad', 3, $1, false)",
        )
        .bind(vec![3u8, 1, 2, 3])
        .execute(&schema.pool)
        .await
        .unwrap();
        drop(schema);

        let db = open(&server, true).await;
        assert_eq!(db.count_legacy_contracts().await.unwrap(), 1);
        assert_eq!(db.get_confirmed_contracts().await.unwrap().len(), 1);

        let report = db.migrate_legacy_contracts().await.unwrap();
        assert_eq!(report.migrated, 0);
        assert_eq!(report.failed.len(), 1);
        assert_eq!(report.failed[0].0, "bad");
        assert!(!report.is_complete());
    }

    /// Every blob in a live legacy database survives the row layout byte for
    /// byte. Read only: it never writes to the database it is pointed at.
    ///
    /// ```sh
    /// DDK_MIGRATION_CHECK_URL=postgres://... cargo test -p ddk --features postgres \
    ///     legacy_blobs_round_trip_against_a_live_database -- --ignored --nocapture
    /// ```
    #[tokio::test]
    #[ignore = "needs DDK_MIGRATION_CHECK_URL"]
    async fn legacy_blobs_round_trip_against_a_live_database() {
        let url = std::env::var("DDK_MIGRATION_CHECK_URL").expect("DDK_MIGRATION_CHECK_URL");
        let pool = PoolOptions::<Postgres>::new()
            .max_connections(1)
            .connect(&url)
            .await
            .unwrap();
        let rows = sqlx::query_as::<Postgres, super::super::sqlx::ContractData>(
            "SELECT id, state, contract_data, is_compressed FROM contract_data ORDER BY id",
        )
        .fetch_all(&pool)
        .await
        .unwrap();
        assert!(!rows.is_empty(), "no legacy rows to check");
        for legacy in &rows {
            let contract = deserialize_contract(&legacy.contract_data)
                .unwrap_or_else(|e| panic!("{}: legacy decode: {e}", legacy.id));
            let row = ContractRow::from_contract(&contract)
                .unwrap_or_else(|e| panic!("{}: to row: {e}", legacy.id));
            assert_eq!(row.id, legacy.id, "row id differs from the legacy id");
            let read = row
                .into_contract()
                .unwrap_or_else(|e| panic!("{}: from row: {e}", legacy.id));
            assert_eq!(
                bytes(&read),
                bytes(&contract),
                "{}: bytes differ",
                legacy.id
            );
            println!("ok {} state={}", legacy.id, legacy.state);
        }
        println!("{} contracts round-trip byte for byte", rows.len());
    }

    /// A stream holding one record of type 65007 with `body` as its one-byte body.
    fn stream_with_record(body: u8) -> ddk_messages::tlv_stream::TlvStream {
        let bytes = [0xfd, 0xfd, 0xef, 0x01, body];
        ddk_messages::tlv_stream::TlvStream::read_to_end(&mut lightning::io::Cursor::new(bytes))
            .unwrap()
    }

    /// TLV records on a legacy blob come through the migration and back out
    /// of the row, on every layer of a signed contract.
    #[tokio::test]
    async fn tlv_records_survive_the_legacy_migration() {
        let server = TestPostgres::start("ddk").await;
        let schema = open(&server, true).await;
        let Contract::Signed(mut signed) = fixture("Signed") else {
            panic!("fixture is not signed")
        };
        signed.accepted_contract.offered_contract.tlvs = stream_with_record(1);
        signed.accepted_contract.tlvs = stream_with_record(2);
        signed.tlvs = stream_with_record(3);
        let contract = Contract::Signed(signed);
        seed_legacy(&schema.pool, &contract).await;
        drop(schema);

        let db = open(&server, true).await;
        assert_eq!(db.count_legacy_contracts().await.unwrap(), 0);
        let Some(Contract::Signed(read)) = db.get_contract(&contract.get_id()).await.unwrap()
        else {
            panic!("contract not found or not signed")
        };
        assert_eq!(
            read.accepted_contract.offered_contract.tlvs,
            stream_with_record(1)
        );
        assert_eq!(read.accepted_contract.tlvs, stream_with_record(2));
        assert_eq!(read.tlvs, stream_with_record(3));
        assert_eq!(bytes(&Contract::Signed(read)), bytes(&contract));
    }

    /// A contract closed by refund has no CET and no attestations. Both
    /// columns are null and the contract still round-trips.
    #[tokio::test]
    async fn closed_by_refund_round_trips() {
        let (_server, db) = seed_db().await;
        let Contract::Closed(mut closed) = fixture("Closed") else {
            panic!("fixture is not closed")
        };
        closed.signed_cet = None;
        closed.attestations = None;
        let contract = Contract::Closed(closed);

        db.update_contract(&contract).await.unwrap();

        let (cet, attestations): (Option<Vec<u8>>, Option<Vec<u8>>) =
            sqlx::query_as("SELECT signed_cet, attestations FROM dlc_contracts WHERE id = $1")
                .bind(hex::encode(contract.get_id()))
                .fetch_one(&db.pool)
                .await
                .unwrap();
        assert!(cet.is_none());
        assert!(attestations.is_none());

        let read = db.get_contract(&contract.get_id()).await.unwrap().unwrap();
        assert_eq!(bytes(&read), bytes(&contract));
        let metadata = db.get_contract_metadata(None).await.unwrap();
        assert_eq!(metadata.len(), 1);
        assert!(metadata[0].cet_txid.is_none());
    }

    /// Offers we received are offers to act on; offers we made are not.
    #[tokio::test]
    async fn contract_offers_are_the_ones_we_received() {
        let server = TestPostgres::start("ddk").await;
        let db = open(&server, true).await;
        let Contract::Offered(mut received) = fixture("Offered") else {
            panic!("fixture is not offered")
        };
        received.is_offer_party = false;
        let mut made = received.clone();
        made.is_offer_party = true;
        made.id = [9u8; 32];

        db.create_contract(&received).await.unwrap();
        db.create_contract(&made).await.unwrap();

        assert_eq!(db.get_contracts().await.unwrap().len(), 2);
        let offers = db.get_contract_offers().await.unwrap();
        assert_eq!(offers.len(), 1);
        assert_eq!(offers[0].id, received.id);
        assert!(!offers[0].is_offer_party);
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

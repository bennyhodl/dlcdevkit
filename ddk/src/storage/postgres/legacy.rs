//! The legacy blob layout: one opaque blob per contract in `contract_data`,
//! with a copy of a few fields in `contract_metadata`.
//!
//! Releases up to 2.0 wrote this layout. It is the version one row format.
//! This module is the version one reader and the migration that moves each
//! blob into a `dlc_contracts` row. Both go away in the release that drops
//! the two legacy tables.

use super::contract_row::ContractRow;
use super::PostgresStore;
use crate::error::to_storage_error;
use crate::logger::{log_error, log_info, log_warn, WriteLog};
use crate::storage::sqlx::ContractData;
use crate::util::ser::deserialize_contract;
use ddk_manager::contract::Contract;
use ddk_manager::error::Error;
use sqlx::Postgres;

/// A blob row that has no `dlc_contracts` row yet.
const LEGACY_ROWS: &str = "SELECT cd.id, cd.state, cd.contract_data, cd.is_compressed
    FROM contract_data cd
    WHERE NOT EXISTS (SELECT 1 FROM dlc_contracts d WHERE d.id = cd.id)";

/// What [`PostgresStore::migrate_legacy_contracts`] did.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct LegacyMigrationReport {
    /// Contracts moved into `dlc_contracts`.
    pub migrated: usize,
    /// Contracts left in the legacy tables, with the error that stopped each.
    pub failed: Vec<(String, String)>,
}

impl LegacyMigrationReport {
    /// True when no contract was left behind.
    pub fn is_complete(&self) -> bool {
        self.failed.is_empty()
    }
}

impl PostgresStore {
    /// Contracts still stored in the legacy blob layout.
    pub async fn count_legacy_contracts(&self) -> Result<u64, Error> {
        let (count,): (i64,) = sqlx::query_as(&format!("SELECT COUNT(*) FROM ({LEGACY_ROWS}) l"))
            .fetch_one(&self.pool)
            .await
            .map_err(to_storage_error)?;
        Ok(count as u64)
    }

    /// Moves every contract still in the legacy blob layout into a
    /// `dlc_contracts` row.
    ///
    /// One transaction per contract: the row is inserted and the legacy rows
    /// are deleted together, so a crash leaves each contract in exactly one
    /// place. A contract that fails to convert stays in the legacy tables,
    /// is reported, and does not stop the others. Safe to run again: it only
    /// selects what is left.
    pub async fn migrate_legacy_contracts(&self) -> Result<LegacyMigrationReport, Error> {
        let rows = sqlx::query_as::<Postgres, ContractData>(LEGACY_ROWS)
            .fetch_all(&self.pool)
            .await
            .map_err(to_storage_error)?;

        let mut report = LegacyMigrationReport::default();
        if rows.is_empty() {
            return Ok(report);
        }
        log_info!(
            self.logger,
            "Migrating contracts from the legacy blob layout. count={}",
            rows.len()
        );

        for legacy in rows {
            match self.migrate_legacy_row(&legacy).await {
                Ok(()) => report.migrated += 1,
                Err(e) => {
                    log_error!(
                        self.logger,
                        "Could not migrate contract from the legacy blob layout. id={} error={}",
                        legacy.id,
                        e
                    );
                    report.failed.push((legacy.id, e.to_string()));
                }
            }
        }

        log_info!(
            self.logger,
            "Finished migrating contracts from the legacy blob layout. migrated={} failed={}",
            report.migrated,
            report.failed.len()
        );
        Ok(report)
    }

    async fn migrate_legacy_row(&self, legacy: &ContractData) -> Result<(), Error> {
        let contract = deserialize_contract(&legacy.contract_data)?;
        let row = ContractRow::from_contract(&contract)?;
        if row.id != legacy.id {
            return Err(Error::StorageError(format!(
                "legacy row id {} does not match the contract id {}",
                legacy.id, row.id
            )));
        }

        let mut tx = self.pool.begin().await.map_err(to_storage_error)?;
        super::upsert_contract_row(&mut tx, &row).await?;
        delete_legacy_rows(&mut tx, &legacy.id).await?;
        tx.commit().await.map_err(to_storage_error)?;
        Ok(())
    }

    /// Logs a warning when contracts are still in the legacy blob layout.
    pub async fn warn_if_legacy_contracts_remain(&self) -> Result<u64, Error> {
        let remaining = self.count_legacy_contracts().await?;
        if remaining > 0 {
            log_warn!(
                self.logger,
                "{} contract(s) are still stored in the legacy blob layout (contract_data). \
                 They still load, but the legacy reader will be removed in a later release. \
                 Run `ddk-node migrate --postgres-url <url>` or \
                 `PostgresStore::migrate_legacy_contracts` to move them to dlc_contracts.",
                remaining
            );
        }
        Ok(remaining)
    }

    /// One legacy contract by id, when it has no `dlc_contracts` row.
    pub(super) async fn legacy_contract(&self, id: &str) -> Result<Option<Contract>, Error> {
        let row =
            sqlx::query_as::<Postgres, ContractData>(&format!("{LEGACY_ROWS} AND cd.id = $1"))
                .bind(id)
                .fetch_optional(&self.pool)
                .await
                .map_err(to_storage_error)?;
        match row {
            Some(row) => {
                self.warn_legacy_read(1);
                Ok(Some(deserialize_contract(&row.contract_data)?))
            }
            None => Ok(None),
        }
    }

    /// The legacy contracts with no `dlc_contracts` row, in `state` when given.
    pub(super) async fn legacy_contracts(
        &self,
        state: Option<i16>,
    ) -> Result<Vec<Contract>, Error> {
        let rows = match state {
            Some(state) => {
                sqlx::query_as::<Postgres, ContractData>(&format!(
                    "{LEGACY_ROWS} AND cd.state = $1"
                ))
                .bind(state)
                .fetch_all(&self.pool)
                .await
            }
            None => {
                sqlx::query_as::<Postgres, ContractData>(LEGACY_ROWS)
                    .fetch_all(&self.pool)
                    .await
            }
        }
        .map_err(to_storage_error)?;
        if rows.is_empty() {
            return Ok(vec![]);
        }
        self.warn_legacy_read(rows.len());
        rows.iter()
            .map(|row| deserialize_contract(&row.contract_data))
            .collect()
    }

    fn warn_legacy_read(&self, count: usize) {
        log_warn!(
            self.logger,
            "Read {} contract(s) from the legacy blob layout. Run `ddk-node migrate` to move them.",
            count
        );
    }
}

/// Deletes the legacy rows of a contract, if any.
pub(super) async fn delete_legacy_rows(
    tx: &mut sqlx::Transaction<'_, Postgres>,
    id: &str,
) -> Result<(), Error> {
    sqlx::query("DELETE FROM contract_data WHERE id = $1")
        .bind(id)
        .execute(&mut **tx)
        .await
        .map_err(to_storage_error)?;
    sqlx::query("DELETE FROM contract_metadata WHERE id = $1")
        .bind(id)
        .execute(&mut **tx)
        .await
        .map_err(to_storage_error)?;
    Ok(())
}

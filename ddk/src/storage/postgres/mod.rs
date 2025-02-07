use super::sqlx::SqlxError;
use crate::{
    storage::sqlx::ContractRow,
    util::ser::{deserialize_contract, serialize_contract},
};
use ddk_manager::contract::{offered_contract::OfferedContract, Contract};
use sqlx::{Database, Pool, Postgres};

/// Manages a pool of database connections.
#[derive(Debug, Clone)]
pub struct Store<DB: Database> {
    pub(crate) pool: Pool<DB>,
}

impl Store<Postgres> {
    pub async fn new(url: &str, migrations: bool) -> Result<Self, SqlxError> {
        let pool = Pool::<Postgres>::connect(url).await?;
        if migrations {
            tracing::info!("Migrating postgres");
            sqlx::migrate!("src/storage/postgres/migrations")
                .run(&pool)
                .await?;
        }
        Ok(Self { pool })
    }

    pub async fn create_offer_contract(&self, contract: &OfferedContract) -> Result<(), SqlxError> {
        sqlx::query_as!(
            ContractRow,
            r#"
           INSERT INTO contracts (
               id, state, is_offer_party, counter_party,
               offer_collateral, accept_collateral, total_collateral, fee_rate_per_vb, 
               cet_locktime, refund_locktime, pnl, contract_data
           )
           VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12)
           RETURNING *
           "#,
            hex::encode(contract.id),
            1 as i16,
            contract.is_offer_party,
            hex::encode(contract.counter_party.serialize()),
            contract.offer_params.collateral as i64,
            (contract.total_collateral - contract.offer_params.collateral) as i64,
            contract.total_collateral as i64,
            contract.fee_rate_per_vb as i64,
            contract.cet_locktime as i32,
            contract.refund_locktime as i32,
            None as Option<i64>,
            serialize_contract(&Contract::Offered(contract.clone()))?
        )
        .fetch_one(&self.pool)
        .await?;

        Ok(())
    }

    pub async fn get_stored_contract(&self, id: &[u8]) -> Result<Contract, SqlxError> {
        let contract = sqlx::query_as!(
            ContractRow,
            "SELECT * FROM contracts WHERE id = $1",
            hex::encode(id)
        )
        .fetch_one(&self.pool)
        .await?;

        Ok(
            deserialize_contract(&contract.contract_data)
                .map_err(SqlxError::DeserializeContract)?,
        )
    }
}

#[cfg(test)]
mod tests {
    use ddk_manager::contract::{offered_contract::OfferedContract, ser::Serializable};

    use super::*;

    fn deserialize_object<T>(serialized: &[u8]) -> T
    where
        T: Serializable,
    {
        let mut cursor = ::lightning::io::Cursor::new(&serialized);
        T::deserialize(&mut cursor).unwrap()
    }

    #[tokio::test]
    async fn postgres() {
        let store = Store::new(
            "postgres://loco:loco@localhost:5432/sons-of-liberty_development",
            true,
        )
        .await
        .unwrap();
        let serialized = include_bytes!("../../../tests/data/dlc_storage/Offered");
        let offered_contract = deserialize_object::<OfferedContract>(&serialized.to_vec());
        let result = store.create_offer_contract(&offered_contract).await;
        assert!(result.is_ok());

        let contract = store.get_stored_contract(&offered_contract.id).await;
        assert!(contract.is_ok());
        assert!(matches!(contract.unwrap(), Contract::Offered(_)));
    }
}

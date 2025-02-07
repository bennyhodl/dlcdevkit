use super::sqlx::SqlxError;
use crate::{
    error::to_storage_error,
    storage::sqlx::ContractRow,
    util::ser::{deserialize_contract, serialize_contract},
};
use ddk_manager::{
    contract::{offered_contract::OfferedContract, Contract},
    Storage,
};
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
}

#[async_trait::async_trait]
impl Storage for Store<Postgres> {
    async fn get_contract(
        &self,
        id: &ddk_manager::ContractId,
    ) -> Result<Option<ddk_manager::contract::Contract>, ddk_manager::error::Error> {
        let contract = sqlx::query_as!(
            ContractRow,
            "SELECT * FROM contracts WHERE id = $1",
            hex::encode(id)
        )
        .fetch_optional(&self.pool)
        .await
        .map_err(to_storage_error)?;

        if let Some(contract) = contract {
            Ok(Some(deserialize_contract(&contract.contract_data)?))
        } else {
            Ok(None)
        }
    }

    async fn get_channel(
        &self,
        _channel_id: &ddk_manager::ChannelId,
    ) -> Result<Option<ddk_manager::channel::Channel>, ddk_manager::error::Error> {
        todo!()
    }

    async fn get_contracts(
        &self,
    ) -> Result<Vec<ddk_manager::contract::Contract>, ddk_manager::error::Error> {
        todo!()
    }

    async fn upsert_channel(
        &self,
        _channel: ddk_manager::channel::Channel,
        _contract: Option<ddk_manager::contract::Contract>,
    ) -> Result<(), ddk_manager::error::Error> {
        todo!()
    }

    async fn delete_channel(
        &self,
        _channel_id: &ddk_manager::ChannelId,
    ) -> Result<(), ddk_manager::error::Error> {
        todo!()
    }

    async fn create_contract(
        &self,
        contract: &OfferedContract,
    ) -> Result<(), ddk_manager::error::Error> {
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
        .await
        .map_err(to_storage_error)?;

        Ok(())
    }

    async fn delete_contract(
        &self,
        _id: &ddk_manager::ContractId,
    ) -> Result<(), ddk_manager::error::Error> {
        todo!()
    }

    async fn update_contract(
        &self,
        _contract: &ddk_manager::contract::Contract,
    ) -> Result<(), ddk_manager::error::Error> {
        todo!()
    }

    async fn get_chain_monitor(
        &self,
    ) -> Result<Option<ddk_manager::chain_monitor::ChainMonitor>, ddk_manager::error::Error> {
        Ok(None)
    }

    async fn get_contract_offers(
        &self,
    ) -> Result<
        Vec<ddk_manager::contract::offered_contract::OfferedContract>,
        ddk_manager::error::Error,
    > {
        todo!()
    }

    async fn get_signed_channels(
        &self,
        _channel_state: Option<ddk_manager::channel::signed_channel::SignedChannelStateType>,
    ) -> Result<Vec<ddk_manager::channel::signed_channel::SignedChannel>, ddk_manager::error::Error>
    {
        todo!()
    }

    async fn get_signed_contracts(
        &self,
    ) -> Result<
        Vec<ddk_manager::contract::signed_contract::SignedContract>,
        ddk_manager::error::Error,
    > {
        todo!()
    }

    async fn get_offered_channels(
        &self,
    ) -> Result<Vec<ddk_manager::channel::offered_channel::OfferedChannel>, ddk_manager::error::Error>
    {
        todo!()
    }

    async fn persist_chain_monitor(
        &self,
        _monitor: &ddk_manager::chain_monitor::ChainMonitor,
    ) -> Result<(), ddk_manager::error::Error> {
        Ok(())
    }

    async fn get_confirmed_contracts(
        &self,
    ) -> Result<
        Vec<ddk_manager::contract::signed_contract::SignedContract>,
        ddk_manager::error::Error,
    > {
        todo!()
    }

    async fn get_preclosed_contracts(
        &self,
    ) -> Result<Vec<ddk_manager::contract::PreClosedContract>, ddk_manager::error::Error> {
        todo!()
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
        let result = store.create_contract(&offered_contract).await;
        assert!(result.is_ok());

        let contract = store.get_contract(&offered_contract.id).await;
        assert!(contract.is_ok());
        assert!(matches!(contract.unwrap().unwrap(), Contract::Offered(_)));
    }
}

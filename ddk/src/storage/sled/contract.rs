use super::{SledStorage, CHAIN_MONITOR_KEY, CHAIN_MONITOR_TREE};
use crate::logger::{log_error, log_info, WriteLog};
use crate::util::ser::{
    deserialize_contract, serialize_contract, ChannelPrefix, ContractPrefix, SignedChannelPrefix,
};
use bitcoin::consensus::ReadExt;
use ddk_manager::chain_monitor::ChainMonitor;
use ddk_manager::channel::accepted_channel::AcceptedChannel;
use ddk_manager::channel::offered_channel::OfferedChannel;
use ddk_manager::channel::signed_channel::{SignedChannel, SignedChannelStateType};
use ddk_manager::channel::{
    Channel, ClosedChannel, ClosedPunishedChannel, ClosingChannel, FailedAccept, FailedSign,
};
use ddk_manager::contract::offered_contract::OfferedContract;
use ddk_manager::contract::ser::Serializable;
use ddk_manager::contract::signed_contract::SignedContract;
use ddk_manager::contract::{Contract, PreClosedContract};
use ddk_manager::{error::Error, ContractId, Storage};
use sled::transaction::{ConflictableTransactionResult, UnabortableTransactionError};
use sled::Transactional;
use std::convert::TryInto;

fn to_storage_error<T>(e: T) -> Error
where
    T: std::fmt::Display,
{
    Error::StorageError(e.to_string())
}

#[async_trait::async_trait]
impl Storage for SledStorage {
    async fn get_contract(&self, contract_id: &ContractId) -> Result<Option<Contract>, Error> {
        match self
            .contract_tree()?
            .get(contract_id)
            .map_err(to_storage_error)?
        {
            Some(res) => Ok(Some(deserialize_contract(&res.to_vec())?)),
            None => Ok(None),
        }
    }

    async fn get_contracts(&self) -> Result<Vec<Contract>, Error> {
        self.contract_tree()?
            .iter()
            .values()
            .map(|x| deserialize_contract(&x.unwrap().to_vec()))
            .collect::<Result<Vec<Contract>, Error>>()
    }

    async fn create_contract(&self, contract: &OfferedContract) -> Result<(), Error> {
        let serialized = serialize_contract(&Contract::Offered(contract.clone()))?;
        self.contract_tree()?
            .insert(contract.id, serialized)
            .map_err(to_storage_error)?;
        Ok(())
    }

    async fn delete_contract(&self, contract_id: &ContractId) -> Result<(), Error> {
        self.contract_tree()?
            .remove(contract_id)
            .map_err(to_storage_error)?;
        Ok(())
    }

    async fn update_contract(&self, contract: &Contract) -> Result<(), Error> {
        log_info!(self.logger, "Updating contract. {:?}", contract);
        let serialized = serialize_contract(contract)?;
        self.contract_tree()?
            .transaction::<_, _, UnabortableTransactionError>(|db| {
                match contract {
                    a @ Contract::Accepted(_) | a @ Contract::Signed(_) => {
                        db.remove(&a.get_temporary_id())?;
                    }
                    _ => {}
                };

                db.insert(&contract.get_id(), serialized.clone())?;
                Ok(())
            })
            .map_err(|e| {
                log_error!(self.logger, "Could not update contract. error={}", e);
                to_storage_error(e)
            })?;
        Ok(())
    }

    async fn get_contract_offers(&self) -> Result<Vec<OfferedContract>, Error> {
        self.get_data_with_prefix(
            &self.contract_tree()?,
            &[ContractPrefix::Offered.into()],
            None,
        )
    }

    async fn get_signed_contracts(&self) -> Result<Vec<SignedContract>, Error> {
        self.get_data_with_prefix(
            &self.contract_tree()?,
            &[ContractPrefix::Signed.into()],
            None,
        )
    }

    async fn get_confirmed_contracts(&self) -> Result<Vec<SignedContract>, Error> {
        self.get_data_with_prefix(
            &self.contract_tree()?,
            &[ContractPrefix::Confirmed.into()],
            None,
        )
    }

    async fn get_preclosed_contracts(&self) -> Result<Vec<PreClosedContract>, Error> {
        self.get_data_with_prefix(
            &self.contract_tree()?,
            &[ContractPrefix::PreClosed.into()],
            None,
        )
    }

    async fn upsert_channel(
        &self,
        channel: Channel,
        contract: Option<Contract>,
    ) -> Result<(), Error> {
        let serialized = serialize_channel(&channel)?;
        let serialized_contract = match contract.as_ref() {
            Some(c) => Some(serialize_contract(c)?),
            None => None,
        };
        let channel_tree = self.channel_tree()?;
        let contract_tree = self.contract_tree()?;
        (&channel_tree, &contract_tree)
            .transaction::<_, ()>(
                |(channel_db, contract_db)| -> ConflictableTransactionResult<(), UnabortableTransactionError> {
                    match &channel {
                        a @ Channel::Accepted(_) | a @ Channel::Signed(_) => {
                            channel_db.remove(&a.get_temporary_id())?;
                        }
                        _ => {}
                    };

                    channel_db.insert(&channel.get_id(), serialized.clone())?;

                    if let Some(c) = contract.as_ref() {
                        insert_contract(
                            contract_db,
                            serialized_contract
                                .clone()
                                .expect("to have the serialized version"),
                            c,
                        )?;
                    }
                    Ok(())
                },
            )
        .map_err(to_storage_error)?;
        Ok(())
    }

    async fn delete_channel(&self, channel_id: &ddk_manager::ChannelId) -> Result<(), Error> {
        self.channel_tree()?
            .remove(channel_id)
            .map_err(to_storage_error)?;
        Ok(())
    }

    async fn get_channel(
        &self,
        channel_id: &ddk_manager::ChannelId,
    ) -> Result<Option<Channel>, Error> {
        match self
            .channel_tree()?
            .get(channel_id)
            .map_err(to_storage_error)?
        {
            Some(res) => Ok(Some(deserialize_channel(&res)?)),
            None => Ok(None),
        }
    }

    async fn get_signed_channels(
        &self,
        channel_state: Option<SignedChannelStateType>,
    ) -> Result<Vec<SignedChannel>, Error> {
        let (prefix, consume) = if let Some(state) = &channel_state {
            (
                vec![
                    ChannelPrefix::Signed.into(),
                    SignedChannelPrefix::get_prefix(state),
                ],
                None,
            )
        } else {
            (vec![ChannelPrefix::Signed.into()], Some(1))
        };

        self.get_data_with_prefix(&self.channel_tree()?, &prefix, consume)
    }

    async fn get_offered_channels(&self) -> Result<Vec<OfferedChannel>, Error> {
        self.get_data_with_prefix(
            &self.channel_tree()?,
            &[ChannelPrefix::Offered.into()],
            None,
        )
    }

    async fn persist_chain_monitor(&self, monitor: &ChainMonitor) -> Result<(), Error> {
        self.open_tree(&[CHAIN_MONITOR_TREE])?
            .insert([CHAIN_MONITOR_KEY], monitor.serialize()?)
            .map_err(|e| Error::StorageError(format!("Error writing chain monitor: {}", e)))?;
        Ok(())
    }
    async fn get_chain_monitor(&self) -> Result<Option<ChainMonitor>, ddk_manager::error::Error> {
        let serialized = self
            .open_tree(&[CHAIN_MONITOR_TREE])?
            .get([CHAIN_MONITOR_KEY])
            .map_err(|e| Error::StorageError(format!("Error reading chain monitor: {}", e)))?;
        let deserialized = match serialized {
            Some(s) => Some(
                ChainMonitor::deserialize(&mut ::lightning::io::Cursor::new(s))
                    .map_err(to_storage_error)?,
            ),
            None => None,
        };
        Ok(deserialized)
    }
}

fn insert_contract(
    db: &sled::transaction::TransactionalTree,
    serialized: Vec<u8>,
    contract: &Contract,
) -> Result<Option<sled::IVec>, UnabortableTransactionError> {
    match contract {
        a @ Contract::Accepted(_) | a @ Contract::Signed(_) => {
            db.remove(&a.get_temporary_id())?;
        }
        _ => {}
    };

    db.insert(&contract.get_id(), serialized)
}

fn serialize_channel(channel: &Channel) -> Result<Vec<u8>, ::lightning::io::Error> {
    let serialized = match channel {
        Channel::Offered(o) => o.serialize(),
        Channel::Accepted(a) => a.serialize(),
        Channel::Signed(s) => s.serialize(),
        Channel::FailedAccept(f) => f.serialize(),
        Channel::FailedSign(f) => f.serialize(),
        Channel::Cancelled(o) => o.serialize(),
        Channel::Closing(c) => c.serialize(),
        Channel::Closed(c) => c.serialize(),
        Channel::CollaborativelyClosed(c) => c.serialize(),
        Channel::CounterClosed(c) => c.serialize(),
        Channel::ClosedPunished(c) => c.serialize(),
    };
    let mut serialized = serialized?;
    let mut res = Vec::with_capacity(serialized.len() + 1);
    res.push(ChannelPrefix::get_prefix(channel));
    if let Channel::Signed(s) = channel {
        res.push(SignedChannelPrefix::get_prefix(&s.state.get_type()))
    }
    res.append(&mut serialized);
    Ok(res)
}

fn deserialize_channel(buff: &sled::IVec) -> Result<Channel, Error> {
    let mut cursor = lightning::io::Cursor::new(buff);
    let mut prefix = [0u8; 1];
    cursor.read_slice(&mut prefix)?;
    let channel_prefix: ChannelPrefix = prefix[0].try_into()?;
    let channel = match channel_prefix {
        ChannelPrefix::Offered => {
            Channel::Offered(OfferedChannel::deserialize(&mut cursor).map_err(to_storage_error)?)
        }
        ChannelPrefix::Accepted => {
            Channel::Accepted(AcceptedChannel::deserialize(&mut cursor).map_err(to_storage_error)?)
        }
        ChannelPrefix::Signed => {
            // Skip the channel state prefix.
            cursor.set_position(cursor.position() + 1);
            Channel::Signed(SignedChannel::deserialize(&mut cursor).map_err(to_storage_error)?)
        }
        ChannelPrefix::FailedAccept => {
            Channel::FailedAccept(FailedAccept::deserialize(&mut cursor).map_err(to_storage_error)?)
        }
        ChannelPrefix::FailedSign => {
            Channel::FailedSign(FailedSign::deserialize(&mut cursor).map_err(to_storage_error)?)
        }
        ChannelPrefix::Cancelled => {
            Channel::Cancelled(OfferedChannel::deserialize(&mut cursor).map_err(to_storage_error)?)
        }
        ChannelPrefix::Closed => {
            Channel::Closed(ClosedChannel::deserialize(&mut cursor).map_err(to_storage_error)?)
        }
        ChannelPrefix::Closing => {
            Channel::Closing(ClosingChannel::deserialize(&mut cursor).map_err(to_storage_error)?)
        }
        ChannelPrefix::CounterClosed => Channel::CounterClosed(
            ClosedChannel::deserialize(&mut cursor).map_err(to_storage_error)?,
        ),
        ChannelPrefix::ClosedPunished => Channel::ClosedPunished(
            ClosedPunishedChannel::deserialize(&mut cursor).map_err(to_storage_error)?,
        ),
        ChannelPrefix::CollaborativelyClosed => Channel::CollaborativelyClosed(
            ClosedChannel::deserialize(&mut cursor).map_err(to_storage_error)?,
        ),
    };
    Ok(channel)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::logger::Logger;
    use std::sync::Arc;

    macro_rules! sled_test {
        ($name: ident, $body: expr) => {
            #[tokio::test]
            async fn $name() {
                let path = format!("{}{}", "tests/data/dlc_storagedb/", std::stringify!($name));
                let logger = Arc::new(Logger::disabled("sled_test".to_string()));
                {
                    let storage = SledStorage::new(&path, logger).expect("Error opening sled DB");
                    #[allow(clippy::redundant_closure_call)]
                    $body(storage).await;
                }
                std::fs::remove_dir_all(path).unwrap();
            }
        };
    }

    sled_test!(
        create_contract_can_be_retrieved,
        |storage: SledStorage| async move {
            let serialized = include_bytes!("../../../../testconfig/contract_binaries/Offered");
            let contract = deserialize_contract(&serialized.to_vec());
            let contract = match contract {
                Ok(c) => {
                    if let Contract::Offered(c) = c {
                        c
                    } else {
                        panic!("Contract is not an offered contract");
                    }
                }
                Err(e) => {
                    panic!("Error deserializing contract: {:?}", e);
                }
            };

            storage
                .create_contract(&contract)
                .await
                .expect("Error creating contract");

            let retrieved = storage
                .get_contract(&contract.id)
                .await
                .expect("Error retrieving contract.");

            if let Some(Contract::Offered(retrieved_offer)) = retrieved {
                assert_eq!(
                    contract.serialize().unwrap()[..],
                    retrieved_offer.serialize().unwrap()[..]
                );
            } else {
                unreachable!();
            }
        }
    );

    async fn insert_offered_signed_and_confirmed(storage: &mut SledStorage) {
        let serialized = include_bytes!("../../../../testconfig/contract_binaries/Offered");
        let offered_contract = deserialize_contract(&serialized.to_vec());
        let offered_contract = match offered_contract {
            Ok(c) => {
                if let Contract::Offered(c) = c {
                    c
                } else {
                    panic!("Contract is not an offered contract");
                }
            }
            Err(e) => {
                panic!("Error deserializing contract: {:?}", e);
            }
        };
        storage
            .create_contract(&offered_contract)
            .await
            .expect("Error creating contract");

        let serialized = include_bytes!("../../../../testconfig/contract_binaries/Signed");
        let contract = deserialize_contract(&serialized.to_vec());
        storage
            .update_contract(&contract.unwrap())
            .await
            .expect("Error creating contract");
        // let serialized = include_bytes!("../../../../testconfig/contract_binaries/Signed1");
        // let signed_contract = Contract::Signed(deserialize_object(serialized));
        // storage
        //     .update_contract(&signed_contract)
        //     .await
        //     .expect("Error creating contract");

        let serialized = include_bytes!("../../../../testconfig/contract_binaries/Confirmed");
        let confirmed_contract = deserialize_contract(&serialized.to_vec()).unwrap();
        storage
            .update_contract(&confirmed_contract)
            .await
            .expect("Error creating contract");
        // let serialized = include_bytes!("../../../tests/data/dlc_storage/Confirmed1");
        // let confirmed_contract = Contract::Confirmed(deserialize_object(serialized));
        // storage
        //     .update_contract(&confirmed_contract)
        //     .await
        //     .expect("Error creating contract");

        let serialized = include_bytes!("../../../../testconfig/contract_binaries/PreClosed");
        let preclosed_contract = deserialize_contract(&serialized.to_vec()).unwrap();
        storage
            .update_contract(&preclosed_contract)
            .await
            .expect("Error creating contract");
    }

    sled_test!(
        update_contract_is_updated,
        |storage: SledStorage| async move {
            let serialized = include_bytes!("../../../../testconfig/contract_binaries/Offered");
            let offered_contract = deserialize_contract(&serialized.to_vec()).unwrap();
            if let Contract::Offered(offered_contract) = offered_contract {
                storage
                    .create_contract(&offered_contract)
                    .await
                    .expect("Error creating contract");
            } else {
                panic!("Contract is not an offered contract");
            }
            let serialized = include_bytes!("../../../../testconfig/contract_binaries/Accepted");
            let accepted_contract = deserialize_contract(&serialized.to_vec()).unwrap();
            if let Contract::Accepted(accepted_contract) = &accepted_contract {
                storage
                    .update_contract(&Contract::Accepted(accepted_contract.clone()))
                    .await
                    .expect("Error updating contract.");
            } else {
                panic!("Contract is not an accepted contract");
            }
            let retrieved = storage
                .get_contract(&accepted_contract.get_id())
                .await
                .expect("Error retrieving contract.");

            if let Some(Contract::Accepted(_)) = retrieved {
            } else {
                unreachable!();
            }
        }
    );

    sled_test!(
        get_signed_contracts_only_signed,
        |mut storage: SledStorage| async move {
            insert_offered_signed_and_confirmed(&mut storage).await;

            let signed_contracts = storage
                .get_signed_contracts()
                .await
                .expect("Error retrieving signed contracts");

            assert_eq!(0, signed_contracts.len());
        }
    );

    sled_test!(
        get_confirmed_contracts_only_confirmed,
        |mut storage: SledStorage| async move {
            insert_offered_signed_and_confirmed(&mut storage).await;

            let confirmed_contracts = storage
                .get_confirmed_contracts()
                .await
                .expect("Error retrieving signed contracts");

            assert_eq!(0, confirmed_contracts.len());
        }
    );

    sled_test!(
        get_offered_contracts_only_offered,
        |mut storage: SledStorage| async move {
            insert_offered_signed_and_confirmed(&mut storage).await;

            let offered_contracts = storage
                .get_contract_offers()
                .await
                .expect("Error retrieving signed contracts");

            assert_eq!(0, offered_contracts.len());
        }
    );

    sled_test!(
        get_preclosed_contracts_only_preclosed,
        |mut storage: SledStorage| async move {
            insert_offered_signed_and_confirmed(&mut storage).await;

            let preclosed_contracts = storage
                .get_preclosed_contracts()
                .await
                .expect("Error retrieving preclosed contracts");

            assert_eq!(1, preclosed_contracts.len());
        }
    );

    sled_test!(
        get_contracts_all_returned,
        |mut storage: SledStorage| async move {
            insert_offered_signed_and_confirmed(&mut storage).await;

            let contracts = storage
                .get_contracts()
                .await
                .expect("Error retrieving contracts");

            assert_eq!(1, contracts.len());
        }
    );

    sled_test!(
        persist_chain_monitor_test,
        |storage: SledStorage| async move {
            let chain_monitor = ChainMonitor::new(123);

            storage
                .persist_chain_monitor(&chain_monitor)
                .await
                .expect("to be able to persist the chain monistor.");

            let retrieved = storage
                .get_chain_monitor()
                .await
                .expect("to be able to retrieve the chain monitor.")
                .expect("to have a persisted chain monitor.");

            assert_eq!(chain_monitor, retrieved);
        }
    );
}

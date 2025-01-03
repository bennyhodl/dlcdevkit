use super::{SledStorage, CHAIN_MONITOR_KEY, CHAIN_MONITOR_TREE};
use crate::util::{deserialize_contract, serialize_contract};
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

macro_rules! convertible_enum {
    (enum $name:ident {
        $($vname:ident $(= $val:expr)?,)*;
        $($tname:ident $(= $tval:expr)?,)*
    }, $input:ident) => {
        #[derive(Debug)]
        enum $name {
            $($vname $(= $val)?,)*
            $($tname $(= $tval)?,)*
        }

        impl From<$name> for u8 {
            fn from(prefix: $name) -> u8 {
                prefix as u8
            }
        }

        impl std::convert::TryFrom<u8> for $name {
            type Error = Error;

            fn try_from(v: u8) -> Result<Self, Self::Error> {
                match v {
                    $(x if x == u8::from($name::$vname) => Ok($name::$vname),)*
                    $(x if x == u8::from($name::$tname) => Ok($name::$tname),)*
                    _ => Err(Error::StorageError("Unknown prefix".to_string())),
                }
            }
        }

        impl $name {
            fn get_prefix(input: &$input) -> u8 {
                let prefix = match input {
                    $($input::$vname(_) => $name::$vname,)*
                    $($input::$tname{..} => $name::$tname,)*
                };
                prefix.into()
            }
        }
    }
}

convertible_enum!(
    enum ContractPrefix {
        Offered = 1,
        Accepted,
        Signed,
        Confirmed,
        PreClosed,
        Closed,
        FailedAccept,
        FailedSign,
        Refunded,
        Rejected,;
    },
    Contract
);

convertible_enum!(
    enum ChannelPrefix {
        Offered = 100,
        Accepted,
        Signed,
        FailedAccept,
        FailedSign,
        Closing,
        Closed,
        CounterClosed,
        ClosedPunished,
        CollaborativelyClosed,
        Cancelled,;
    },
    Channel
);

convertible_enum!(
    enum SignedChannelPrefix {;
        Established = 1,
        SettledOffered,
        SettledReceived,
        SettledAccepted,
        SettledConfirmed,
        Settled,
        Closing,
        CollaborativeCloseOffered,
        RenewAccepted,
        RenewOffered,
        RenewFinalized,
        RenewConfirmed,
    },
    SignedChannelStateType
);

fn to_storage_error<T>(e: T) -> Error
where
    T: std::fmt::Display,
{
    Error::StorageError(e.to_string())
}

impl Storage for SledStorage {
    fn get_contract(&self, contract_id: &ContractId) -> Result<Option<Contract>, Error> {
        match self
            .contract_tree()?
            .get(contract_id)
            .map_err(to_storage_error)?
        {
            Some(res) => Ok(Some(deserialize_contract(&res.to_vec())?)),
            None => Ok(None),
        }
    }

    fn get_contracts(&self) -> Result<Vec<Contract>, Error> {
        self.contract_tree()?
            .iter()
            .values()
            .map(|x| deserialize_contract(&x.unwrap().to_vec()))
            .collect::<Result<Vec<Contract>, Error>>()
    }

    fn create_contract(&self, contract: &OfferedContract) -> Result<(), Error> {
        let serialized = serialize_contract(&Contract::Offered(contract.clone()))?;
        self.contract_tree()?
            .insert(contract.id, serialized)
            .map_err(to_storage_error)?;
        Ok(())
    }

    fn delete_contract(&self, contract_id: &ContractId) -> Result<(), Error> {
        self.contract_tree()?
            .remove(contract_id)
            .map_err(to_storage_error)?;
        Ok(())
    }

    fn update_contract(&self, contract: &Contract) -> Result<(), Error> {
        tracing::info!("Updating contract. {:?}", contract);
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
                tracing::error!("Could not update contract: {:?}", e);
                to_storage_error(e)
            })?;
        Ok(())
    }

    fn get_contract_offers(&self) -> Result<Vec<OfferedContract>, Error> {
        self.get_data_with_prefix(
            &self.contract_tree()?,
            &[ContractPrefix::Offered.into()],
            None,
        )
    }

    fn get_signed_contracts(&self) -> Result<Vec<SignedContract>, Error> {
        self.get_data_with_prefix(
            &self.contract_tree()?,
            &[ContractPrefix::Signed.into()],
            None,
        )
    }

    fn get_confirmed_contracts(&self) -> Result<Vec<SignedContract>, Error> {
        self.get_data_with_prefix(
            &self.contract_tree()?,
            &[ContractPrefix::Confirmed.into()],
            None,
        )
    }

    fn get_preclosed_contracts(&self) -> Result<Vec<PreClosedContract>, Error> {
        self.get_data_with_prefix(
            &self.contract_tree()?,
            &[ContractPrefix::PreClosed.into()],
            None,
        )
    }

    fn upsert_channel(&self, channel: Channel, contract: Option<Contract>) -> Result<(), Error> {
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

    fn delete_channel(&self, channel_id: &ddk_manager::ChannelId) -> Result<(), Error> {
        self.channel_tree()?
            .remove(channel_id)
            .map_err(to_storage_error)?;
        Ok(())
    }

    fn get_channel(&self, channel_id: &ddk_manager::ChannelId) -> Result<Option<Channel>, Error> {
        match self
            .channel_tree()?
            .get(channel_id)
            .map_err(to_storage_error)?
        {
            Some(res) => Ok(Some(deserialize_channel(&res)?)),
            None => Ok(None),
        }
    }

    fn get_signed_channels(
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

    fn get_offered_channels(&self) -> Result<Vec<OfferedChannel>, Error> {
        self.get_data_with_prefix(
            &self.channel_tree()?,
            &[ChannelPrefix::Offered.into()],
            None,
        )
    }

    fn persist_chain_monitor(&self, monitor: &ChainMonitor) -> Result<(), Error> {
        self.open_tree(&[CHAIN_MONITOR_TREE])?
            .insert([CHAIN_MONITOR_KEY], monitor.serialize()?)
            .map_err(|e| Error::StorageError(format!("Error writing chain monitor: {}", e)))?;
        Ok(())
    }
    fn get_chain_monitor(&self) -> Result<Option<ChainMonitor>, ddk_manager::error::Error> {
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

    macro_rules! sled_test {
        ($name: ident, $body: expr) => {
            #[test]
            fn $name() {
                let path = format!("{}{}", "tests/data/dlc_storagedb/", std::stringify!($name));
                {
                    let storage = SledStorage::new(&path).expect("Error opening sled DB");
                    #[allow(clippy::redundant_closure_call)]
                    $body(storage);
                }
                std::fs::remove_dir_all(path).unwrap();
            }
        };
    }

    fn deserialize_object<T>(serialized: &[u8]) -> T
    where
        T: Serializable,
    {
        let mut cursor = ::lightning::io::Cursor::new(&serialized);
        T::deserialize(&mut cursor).unwrap()
    }

    sled_test!(create_contract_can_be_retrieved, |storage: SledStorage| {
        let serialized = include_bytes!("../../../tests/data/dlc_storage/Offered");
        let contract = deserialize_object(serialized);

        storage
            .create_contract(&contract)
            .expect("Error creating contract");

        let retrieved = storage
            .get_contract(&contract.id)
            .expect("Error retrieving contract.");

        if let Some(Contract::Offered(retrieved_offer)) = retrieved {
            assert_eq!(serialized[..], retrieved_offer.serialize().unwrap()[..]);
        } else {
            unreachable!();
        }
    });

    sled_test!(update_contract_is_updated, |storage: SledStorage| {
        let serialized = include_bytes!("../../../tests/data/dlc_storage/Offered");
        let offered_contract = deserialize_object(serialized);
        let serialized = include_bytes!("../../../tests/data/dlc_storage/Accepted");
        let accepted_contract = deserialize_object(serialized);
        let accepted_contract = Contract::Accepted(accepted_contract);

        storage
            .create_contract(&offered_contract)
            .expect("Error creating contract");

        storage
            .update_contract(&accepted_contract)
            .expect("Error updating contract.");
        let retrieved = storage
            .get_contract(&accepted_contract.get_id())
            .expect("Error retrieving contract.");

        if let Some(Contract::Accepted(_)) = retrieved {
        } else {
            unreachable!();
        }
    });

    sled_test!(delete_contract_is_deleted, |storage: SledStorage| {
        let serialized = include_bytes!("../../../tests/data/dlc_storage/Offered");
        let contract = deserialize_object(serialized);
        storage
            .create_contract(&contract)
            .expect("Error creating contract");

        storage
            .delete_contract(&contract.id)
            .expect("Error deleting contract");

        assert!(storage
            .get_contract(&contract.id)
            .expect("Error querying contract")
            .is_none());
    });

    fn insert_offered_signed_and_confirmed(storage: &mut SledStorage) {
        let serialized = include_bytes!("../../../tests/data/dlc_storage/Offered");
        let offered_contract = deserialize_object(serialized);
        storage
            .create_contract(&offered_contract)
            .expect("Error creating contract");

        let serialized = include_bytes!("../../../tests/data/dlc_storage/Signed");
        let signed_contract = Contract::Signed(deserialize_object(serialized));
        storage
            .update_contract(&signed_contract)
            .expect("Error creating contract");
        let serialized = include_bytes!("../../../tests/data/dlc_storage/Signed1");
        let signed_contract = Contract::Signed(deserialize_object(serialized));
        storage
            .update_contract(&signed_contract)
            .expect("Error creating contract");

        let serialized = include_bytes!("../../../tests/data/dlc_storage/Confirmed");
        let confirmed_contract = Contract::Confirmed(deserialize_object(serialized));
        storage
            .update_contract(&confirmed_contract)
            .expect("Error creating contract");
        let serialized = include_bytes!("../../../tests/data/dlc_storage/Confirmed1");
        let confirmed_contract = Contract::Confirmed(deserialize_object(serialized));
        storage
            .update_contract(&confirmed_contract)
            .expect("Error creating contract");

        let serialized = include_bytes!("../../../tests/data/dlc_storage/PreClosed");
        let preclosed_contract = Contract::PreClosed(deserialize_object(serialized));
        storage
            .update_contract(&preclosed_contract)
            .expect("Error creating contract");
    }

    fn insert_offered_and_signed_channels(storage: &mut SledStorage) {
        let serialized = include_bytes!("../../../tests/data/dlc_storage/Offered");
        let offered_contract = deserialize_object(serialized);
        let serialized = include_bytes!("../../../tests/data/dlc_storage/OfferedChannel");
        let offered_channel = deserialize_object(serialized);
        storage
            .upsert_channel(
                Channel::Offered(offered_channel),
                Some(Contract::Offered(offered_contract)),
            )
            .expect("Error creating contract");

        let serialized = include_bytes!("../../../tests/data/dlc_storage/SignedChannelEstablished");
        let signed_channel = Channel::Signed(deserialize_object(serialized));
        storage
            .upsert_channel(signed_channel, None)
            .expect("Error creating contract");

        let serialized = include_bytes!("../../../tests/data/dlc_storage/SignedChannelSettled");
        let signed_channel = Channel::Signed(deserialize_object(serialized));
        storage
            .upsert_channel(signed_channel, None)
            .expect("Error creating contract");
    }

    sled_test!(
        get_signed_contracts_only_signed,
        |mut storage: SledStorage| {
            insert_offered_signed_and_confirmed(&mut storage);

            let signed_contracts = storage
                .get_signed_contracts()
                .expect("Error retrieving signed contracts");

            assert_eq!(2, signed_contracts.len());
        }
    );

    sled_test!(
        get_confirmed_contracts_only_confirmed,
        |mut storage: SledStorage| {
            insert_offered_signed_and_confirmed(&mut storage);

            let confirmed_contracts = storage
                .get_confirmed_contracts()
                .expect("Error retrieving signed contracts");

            assert_eq!(2, confirmed_contracts.len());
        }
    );

    sled_test!(
        get_offered_contracts_only_offered,
        |mut storage: SledStorage| {
            insert_offered_signed_and_confirmed(&mut storage);

            let offered_contracts = storage
                .get_contract_offers()
                .expect("Error retrieving signed contracts");

            assert_eq!(1, offered_contracts.len());
        }
    );

    sled_test!(
        get_preclosed_contracts_only_preclosed,
        |mut storage: SledStorage| {
            insert_offered_signed_and_confirmed(&mut storage);

            let preclosed_contracts = storage
                .get_preclosed_contracts()
                .expect("Error retrieving preclosed contracts");

            assert_eq!(1, preclosed_contracts.len());
        }
    );
    sled_test!(get_contracts_all_returned, |mut storage: SledStorage| {
        insert_offered_signed_and_confirmed(&mut storage);

        let contracts = storage.get_contracts().expect("Error retrieving contracts");

        assert_eq!(6, contracts.len());
    });

    sled_test!(persist_chain_monitor_test, |storage: SledStorage| {
        let chain_monitor = ChainMonitor::new(123);

        storage
            .persist_chain_monitor(&chain_monitor)
            .expect("to be able to persist the chain monistor.");

        let retrieved = storage
            .get_chain_monitor()
            .expect("to be able to retrieve the chain monitor.")
            .expect("to have a persisted chain monitor.");

        assert_eq!(chain_monitor, retrieved);
    });
}

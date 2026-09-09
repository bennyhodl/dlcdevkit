use super::SledStorage;
use crate::logger::{log_error, log_info, WriteLog};
use crate::util::ser::{deserialize_contract, serialize_contract, ContractPrefix};
use ddk_manager::contract::offered_contract::OfferedContract;
use ddk_manager::contract::signed_contract::SignedContract;
use ddk_manager::contract::{Contract, PreClosedContract};
use ddk_manager::{error::Error, ContractId, Storage};
use sled::transaction::UnabortableTransactionError;

fn to_storage_error<T>(e: T) -> Error
where
    T: std::fmt::Display,
{
    Error::StorageError(e.to_string())
}

impl SledStorage {
    /// Contracts in the given state, decoded through [`deserialize_contract`]
    /// so the store never parses the blob layout itself.
    fn get_contracts_in_state<T>(
        &self,
        state: ContractPrefix,
        extract: fn(Contract) -> Option<T>,
    ) -> Result<Vec<T>, Error> {
        let state = u8::from(state);
        Ok(self
            .contract_tree()?
            .iter()
            .values()
            .filter_map(|res| {
                let value = res.unwrap().to_vec();
                if ContractPrefix::peek(&value).ok()? != state {
                    return None;
                }
                extract(deserialize_contract(&value).ok()?)
            })
            .collect())
    }
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
        self.get_contracts_in_state(ContractPrefix::Offered, |c| match c {
            Contract::Offered(o) => Some(o),
            _ => None,
        })
    }

    async fn get_signed_contracts(&self) -> Result<Vec<SignedContract>, Error> {
        self.get_contracts_in_state(ContractPrefix::Signed, |c| match c {
            Contract::Signed(s) => Some(s),
            _ => None,
        })
    }

    async fn get_confirmed_contracts(&self) -> Result<Vec<SignedContract>, Error> {
        self.get_contracts_in_state(ContractPrefix::Confirmed, |c| match c {
            Contract::Confirmed(s) => Some(s),
            _ => None,
        })
    }

    async fn get_preclosed_contracts(&self) -> Result<Vec<PreClosedContract>, Error> {
        self.get_contracts_in_state(ContractPrefix::PreClosed, |c| match c {
            Contract::PreClosed(p) => Some(p),
            _ => None,
        })
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

#[cfg(test)]
mod tests {
    use ddk_manager::contract::ser::Serializable;

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

    #[test]
    fn old_format_offered_contract_deserializes() {
        let serialized = include_bytes!("../../../../testconfig/contract_binaries/old/Offered");
        let contract = deserialize_contract(&serialized.to_vec())
            .expect("Old format Offered binary should deserialize");
        if let Contract::Offered(offered) = contract {
            assert_eq!(offered.contract_flags, 0);
        } else {
            panic!("Expected Offered contract");
        }
    }

    #[test]
    fn new_format_offered_contract_deserializes() {
        let serialized = include_bytes!("../../../../testconfig/contract_binaries/Offered");
        let contract = deserialize_contract(&serialized.to_vec())
            .expect("New format Offered binary should deserialize");
        if let Contract::Offered(offered) = contract {
            assert_eq!(offered.contract_flags, 0);
        } else {
            panic!("Expected Offered contract");
        }
    }
}

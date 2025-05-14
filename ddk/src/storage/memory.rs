use crate::Storage;
use bdk_chain::Merge;
use ddk_manager::{channel::Channel, contract::Contract, ChannelId, ContractId};
use std::collections::HashMap;
use std::sync::RwLock;

#[derive(Default, Debug)]
pub struct MemoryStorage {
    bdk_data: RwLock<Option<bdk_wallet::ChangeSet>>,
    contracts: RwLock<HashMap<ContractId, Contract>>,
    channels: RwLock<HashMap<ChannelId, Channel>>,
    chain_monitor: RwLock<Option<ddk_manager::chain_monitor::ChainMonitor>>,
}

impl MemoryStorage {
    pub fn new() -> Self {
        Self {
            bdk_data: RwLock::new(None),
            contracts: RwLock::new(HashMap::new()),
            channels: RwLock::new(HashMap::new()),
            chain_monitor: RwLock::new(None),
        }
    }
}

#[async_trait::async_trait]
impl Storage for MemoryStorage {
    async fn persist_bdk(
        &self,
        changeset: &bdk_wallet::ChangeSet,
    ) -> Result<(), crate::error::WalletError> {
        let mut persisted_changeset = self.bdk_data.read().unwrap().clone().unwrap_or_default();
        persisted_changeset.merge(changeset.clone());
        *self.bdk_data.write().unwrap() = Some(persisted_changeset);
        Ok(())
    }

    async fn initialize_bdk(&self) -> Result<bdk_wallet::ChangeSet, crate::error::WalletError> {
        Ok(self.bdk_data.read().unwrap().clone().unwrap_or_default())
    }
}

#[async_trait::async_trait]
impl ddk_manager::Storage for MemoryStorage {
    async fn get_contract(
        &self,
        id: &ddk_manager::ContractId,
    ) -> Result<Option<ddk_manager::contract::Contract>, ddk_manager::error::Error> {
        Ok(self.contracts.read().unwrap().get(id).cloned())
    }

    async fn get_channel(
        &self,
        channel_id: &ddk_manager::ChannelId,
    ) -> Result<Option<ddk_manager::channel::Channel>, ddk_manager::error::Error> {
        Ok(self.channels.read().unwrap().get(channel_id).cloned())
    }

    async fn get_contracts(
        &self,
    ) -> Result<Vec<ddk_manager::contract::Contract>, ddk_manager::error::Error> {
        Ok(self.contracts.read().unwrap().values().cloned().collect())
    }

    async fn upsert_channel(
        &self,
        channel: ddk_manager::channel::Channel,
        contract: Option<ddk_manager::contract::Contract>,
    ) -> Result<(), ddk_manager::error::Error> {
        if let Some(contract) = contract {
            self.contracts
                .write()
                .unwrap()
                .insert(contract.get_id(), contract);
        }
        self.channels
            .write()
            .unwrap()
            .insert(channel.get_id(), channel);
        Ok(())
    }

    async fn delete_channel(
        &self,
        channel_id: &ddk_manager::ChannelId,
    ) -> Result<(), ddk_manager::error::Error> {
        self.channels.write().unwrap().remove(channel_id);
        Ok(())
    }

    async fn create_contract(
        &self,
        contract: &ddk_manager::contract::offered_contract::OfferedContract,
    ) -> Result<(), ddk_manager::error::Error> {
        self.contracts
            .write()
            .unwrap()
            .insert(contract.id, Contract::Offered(contract.clone()));
        Ok(())
    }

    async fn delete_contract(
        &self,
        id: &ddk_manager::ContractId,
    ) -> Result<(), ddk_manager::error::Error> {
        self.contracts.write().unwrap().remove(id);
        Ok(())
    }

    async fn update_contract(
        &self,
        contract: &ddk_manager::contract::Contract,
    ) -> Result<(), ddk_manager::error::Error> {
        self.contracts
            .write()
            .unwrap()
            .insert(contract.get_id(), contract.clone());
        Ok(())
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
        let contracts = self.contracts.read().unwrap();
        let offers = contracts
            .values()
            .filter_map(|c| match c {
                Contract::Offered(c) => Some(c),
                _ => None,
            })
            .cloned()
            .collect();
        Ok(offers)
    }

    async fn get_signed_channels(
        &self,
        _channel_state: Option<ddk_manager::channel::signed_channel::SignedChannelStateType>,
    ) -> Result<Vec<ddk_manager::channel::signed_channel::SignedChannel>, ddk_manager::error::Error>
    {
        let channels = self.channels.read().unwrap();
        Ok(channels
            .values()
            .filter_map(|c| match c {
                Channel::Signed(sc) => Some(sc.clone()),
                _ => None,
            })
            .collect())
    }

    async fn get_signed_contracts(
        &self,
    ) -> Result<
        Vec<ddk_manager::contract::signed_contract::SignedContract>,
        ddk_manager::error::Error,
    > {
        let contracts = self.contracts.read().unwrap();
        Ok(contracts
            .values()
            .filter_map(|c| match c {
                Contract::Signed(sc) => Some(sc.clone()),
                _ => None,
            })
            .collect())
    }

    async fn get_offered_channels(
        &self,
    ) -> Result<Vec<ddk_manager::channel::offered_channel::OfferedChannel>, ddk_manager::error::Error>
    {
        let channels = self.channels.read().unwrap();
        Ok(channels
            .values()
            .filter_map(|c| match c {
                Channel::Offered(oc) => Some(oc.clone()),
                _ => None,
            })
            .collect())
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
        let contracts = self.contracts.read().unwrap();
        Ok(contracts
            .values()
            .filter_map(|c| match c {
                Contract::Confirmed(sc) => Some(sc.clone()),
                _ => None,
            })
            .collect())
    }

    async fn get_preclosed_contracts(
        &self,
    ) -> Result<Vec<ddk_manager::contract::PreClosedContract>, ddk_manager::error::Error> {
        let contracts = self.contracts.read().unwrap();
        Ok(contracts
            .values()
            .filter_map(|c| match c {
                Contract::PreClosed(pc) => Some(pc.clone()),
                _ => None,
            })
            .collect())
    }
}

use crate::transport::PeerInformation;
use crate::Storage;
use bdk_chain::Merge;
use ddk_manager::{channel::Channel, contract::Contract, ChannelId, ContractId};
use dlc_messages::oracle_msgs::OracleAnnouncement;
use std::collections::HashMap;
use std::sync::RwLock;

#[derive(Default, Debug)]
pub struct MemoryStorage {
    peers: RwLock<HashMap<String, PeerInformation>>,
    bdk_data: RwLock<Option<bdk_wallet::ChangeSet>>,
    announcements: RwLock<Vec<OracleAnnouncement>>,
    contracts: RwLock<HashMap<ContractId, Contract>>,
    channels: RwLock<HashMap<ChannelId, Channel>>,
    chain_monitor: RwLock<Option<ddk_manager::chain_monitor::ChainMonitor>>,
}

impl MemoryStorage {
    pub fn new() -> Self {
        Self {
            peers: RwLock::new(HashMap::new()),
            bdk_data: RwLock::new(None),
            announcements: RwLock::new(Vec::new()),
            contracts: RwLock::new(HashMap::new()),
            channels: RwLock::new(HashMap::new()),
            chain_monitor: RwLock::new(None),
        }
    }
}

impl Storage for MemoryStorage {
    fn save_peer(&self, _peer: PeerInformation) -> anyhow::Result<()> {
        // self.peers.write().unwrap().insert(peer.id.clone(), peer);
        Ok(())
    }

    fn list_peers(&self) -> anyhow::Result<Vec<PeerInformation>> {
        // Ok(self.peers.read().unwrap().values().cloned().collect())
        Ok(vec![PeerInformation {
            pubkey: "".to_string(),
            host: "".to_string(),
        }])
    }

    fn persist_bdk(
        &self,
        changeset: &bdk_wallet::ChangeSet,
    ) -> Result<(), crate::error::WalletError> {
        let mut persisted_changeset = self.bdk_data.read().unwrap().clone().unwrap_or_default();
        persisted_changeset.merge(changeset.clone());
        *self.bdk_data.write().unwrap() = Some(persisted_changeset);
        Ok(())
    }

    fn initialize_bdk(&self) -> Result<bdk_wallet::ChangeSet, crate::error::WalletError> {
        Ok(self.bdk_data.read().unwrap().clone().unwrap_or_default())
    }

    fn save_announcement(&self, announcement: kormir::OracleAnnouncement) -> anyhow::Result<()> {
        self.announcements.write().unwrap().push(announcement);
        Ok(())
    }

    fn get_marketplace_announcements(&self) -> anyhow::Result<Vec<kormir::OracleAnnouncement>> {
        Ok(self.announcements.read().unwrap().clone())
    }
}

impl ddk_manager::Storage for MemoryStorage {
    fn get_contract(
        &self,
        id: &ddk_manager::ContractId,
    ) -> Result<Option<ddk_manager::contract::Contract>, ddk_manager::error::Error> {
        Ok(self.contracts.read().unwrap().get(id).cloned())
    }

    fn get_channel(
        &self,
        channel_id: &ddk_manager::ChannelId,
    ) -> Result<Option<ddk_manager::channel::Channel>, ddk_manager::error::Error> {
        Ok(self.channels.read().unwrap().get(channel_id).cloned())
    }

    fn get_contracts(
        &self,
    ) -> Result<Vec<ddk_manager::contract::Contract>, ddk_manager::error::Error> {
        Ok(self.contracts.read().unwrap().values().cloned().collect())
    }

    fn upsert_channel(
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

    fn delete_channel(
        &self,
        channel_id: &ddk_manager::ChannelId,
    ) -> Result<(), ddk_manager::error::Error> {
        self.channels.write().unwrap().remove(channel_id);
        Ok(())
    }

    fn create_contract(
        &self,
        contract: &ddk_manager::contract::offered_contract::OfferedContract,
    ) -> Result<(), ddk_manager::error::Error> {
        self.contracts
            .write()
            .unwrap()
            .insert(contract.id, Contract::Offered(contract.clone()));
        Ok(())
    }

    fn delete_contract(
        &self,
        id: &ddk_manager::ContractId,
    ) -> Result<(), ddk_manager::error::Error> {
        self.contracts.write().unwrap().remove(id);
        Ok(())
    }

    fn update_contract(
        &self,
        contract: &ddk_manager::contract::Contract,
    ) -> Result<(), ddk_manager::error::Error> {
        self.contracts
            .write()
            .unwrap()
            .insert(contract.get_id(), contract.clone());
        Ok(())
    }

    fn get_chain_monitor(
        &self,
    ) -> Result<Option<ddk_manager::chain_monitor::ChainMonitor>, ddk_manager::error::Error> {
        Ok(None)
    }

    fn get_contract_offers(
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

    fn get_signed_channels(
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

    fn get_signed_contracts(
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

    fn get_offered_channels(
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

    fn persist_chain_monitor(
        &self,
        _monitor: &ddk_manager::chain_monitor::ChainMonitor,
    ) -> Result<(), ddk_manager::error::Error> {
        Ok(())
    }

    fn get_confirmed_contracts(
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

    fn get_preclosed_contracts(
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

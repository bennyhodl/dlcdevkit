use crate::Storage;
use bdk_chain::Merge;
use ddk_manager::{channel::Channel, contract::Contract, ChannelId, ContractId};
use std::collections::HashMap;
use std::sync::RwLock;

#[derive(Default, Debug)]
pub struct MemoryStorage {
    bdk_data: RwLock<Option<bdk_wallet::ChangeSet>>,
    contract_tracker: RwLock<crate::wallet::contract_tracker::ChangeSet>,
    labels: RwLock<std::collections::BTreeMap<String, bip329::Label>>,
    contracts: RwLock<HashMap<ContractId, Contract>>,
    channels: RwLock<HashMap<ChannelId, Channel>>,
}

impl MemoryStorage {
    pub fn new() -> Self {
        Self::default()
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

    async fn initialize_contract_tracker(
        &self,
    ) -> Result<crate::wallet::contract_tracker::ChangeSet, crate::error::WalletError> {
        Ok(self.contract_tracker.read().unwrap().clone())
    }

    async fn persist_contract_tracker(
        &self,
        changeset: &crate::wallet::contract_tracker::ChangeSet,
    ) -> Result<(), crate::error::WalletError> {
        self.contract_tracker
            .write()
            .unwrap()
            .merge(changeset.clone());
        Ok(())
    }

    async fn load_labels(&self) -> Result<bip329::Labels, crate::error::WalletError> {
        Ok(bip329::Labels::new(
            self.labels.read().unwrap().values().cloned().collect(),
        ))
    }

    async fn persist_label(&self, label: &bip329::Label) -> Result<(), crate::error::WalletError> {
        self.labels
            .write()
            .unwrap()
            .insert(super::label_key(&label.ref_()), label.clone());
        Ok(())
    }

    async fn delete_label(
        &self,
        label_ref: &bip329::LabelRef,
    ) -> Result<(), crate::error::WalletError> {
        self.labels
            .write()
            .unwrap()
            .remove(&super::label_key(label_ref));
        Ok(())
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

#[cfg(test)]
mod tests {
    use super::*;
    use bitcoin::hashes::Hash;

    #[tokio::test]
    async fn labels_round_trip() {
        let storage = MemoryStorage::new();
        let txid = bitcoin::Txid::from_byte_array([0xAB; 32]);

        let label = bip329::Label::Transaction(bip329::TransactionRecord {
            ref_: txid,
            label: Some("DLC funding".to_string()),
            origin: None,
        });
        storage.persist_label(&label).await.unwrap();

        // An input and an output record share the same outpoint
        // reference and must not overwrite each other.
        let outpoint = bitcoin::OutPoint { txid, vout: 0 };
        let output_label = bip329::Label::Output(bip329::OutputRecord {
            ref_: outpoint,
            label: Some("collateral".to_string()),
            spendable: Some(false),
        });
        let input_label = bip329::Label::Input(bip329::InputRecord {
            ref_: outpoint,
            label: Some("funding input".to_string()),
        });
        storage.persist_label(&output_label).await.unwrap();
        storage.persist_label(&input_label).await.unwrap();

        let labels = storage.load_labels().await.unwrap();
        assert_eq!(labels.iter().count(), 3);

        // Replacing by reference and deleting.
        let renamed = bip329::Label::Transaction(bip329::TransactionRecord {
            ref_: txid,
            label: Some("DLC funding renamed".to_string()),
            origin: None,
        });
        storage.persist_label(&renamed).await.unwrap();
        storage.delete_label(&input_label.ref_()).await.unwrap();

        let labels = storage.load_labels().await.unwrap();
        assert_eq!(labels.iter().count(), 2);
        assert!(labels.iter().any(|label| matches!(
            label,
            bip329::Label::Transaction(record) if record.label.as_deref() == Some("DLC funding renamed")
        )));
    }
}

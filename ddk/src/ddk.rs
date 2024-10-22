use crate::chain::EsploraClient;
#[cfg(feature = "marketplace")]
use crate::nostr::marketplace::*;
use crate::wallet::DlcDevKitWallet;
use crate::{Oracle, Storage, Transport};
use anyhow::anyhow;
use bitcoin::secp256k1::PublicKey;
use bitcoin::Network;
use crossbeam::channel::{unbounded, Receiver, Sender};
use dlc_manager::error::Error;
use dlc_manager::{
    contract::contract_input::ContractInput, CachedContractSignerProvider, ContractId,
    SimpleSigner, SystemTimeProvider,
};
use dlc_messages::oracle_msgs::OracleAnnouncement;
use dlc_messages::{AcceptDlc, Message, OfferDlc};
use std::sync::{Arc, RwLock};
use std::time::Duration;
use tokio::runtime::Runtime;

/// DlcDevKit type alias for the [dlc_manager::manager::Manager]
pub type DlcDevKitDlcManager<S, O> = dlc_manager::manager::Manager<
    Arc<DlcDevKitWallet>,
    Arc<CachedContractSignerProvider<Arc<DlcDevKitWallet>, SimpleSigner>>,
    Arc<EsploraClient>,
    Arc<S>,
    Arc<O>,
    Arc<SystemTimeProvider>,
    Arc<DlcDevKitWallet>,
    SimpleSigner,
>;

#[derive(Debug)]
pub enum DlcManagerMessage {
    AcceptDlc {
        contract: ContractId,
        responder: Sender<Result<(ContractId, PublicKey, AcceptDlc), Error>>,
    },
    OfferDlc {
        contract_input: ContractInput,
        counter_party: PublicKey,
        oracle_announcements: Vec<OracleAnnouncement>,
        responder: Sender<OfferDlc>,
    },
    PeriodicCheck,
}

pub struct DlcDevKit<T: Transport, S: Storage, O: Oracle> {
    pub runtime: Arc<RwLock<Option<Runtime>>>,
    pub wallet: Arc<DlcDevKitWallet>,
    pub manager: Arc<DlcDevKitDlcManager<S, O>>,
    pub sender: Arc<Sender<DlcManagerMessage>>,
    pub receiver: Arc<Receiver<DlcManagerMessage>>,
    pub transport: Arc<T>,
    pub storage: Arc<S>,
    pub oracle: Arc<O>,
    pub network: Network,
}

impl<T, S, O> DlcDevKit<T, S, O>
where
    T: Transport,
    S: Storage,
    O: Oracle,
{
    pub fn start(&self) -> anyhow::Result<()> {
        let mut runtime_lock = self.runtime.write().unwrap();

        if runtime_lock.is_some() {
            return Err(anyhow!("DDK is still running."));
        }

        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()?;

        let manager_clone = self.manager.clone();
        let receiver_clone = self.receiver.clone();
        runtime.spawn(async move { Self::run_manager(manager_clone, receiver_clone).await });

        let transport_clone = self.transport.clone();
        runtime.spawn(async move {
            transport_clone.listen().await;
        });

        let transport_clone = self.transport.clone();
        let manager_clone = self.manager.clone();
        runtime.spawn(async move {
            transport_clone.receive_messages(manager_clone).await;
        });

        let wallet_clone = self.wallet.clone();
        runtime.spawn(async move {
            let mut timer = tokio::time::interval(Duration::from_secs(10));
            loop {
                timer.tick().await;
                wallet_clone.sync().unwrap();
            }
        });

        let processor = self.sender.clone();
        runtime.spawn(async move {
            let mut timer = tokio::time::interval(Duration::from_secs(5));
            loop {
                timer.tick().await;
                processor
                    .send(DlcManagerMessage::PeriodicCheck)
                    .expect("couldn't send periodic check");
            }
        });

        #[cfg(feature = "marketplace")]
        let storage_clone = self.storage.clone();
        #[cfg(feature = "marketplace")]
        runtime.spawn(async move {
            tracing::info!("Starting marketplace listener.");
            marketplace_listener(&storage_clone, vec!["ws://127.0.0.1:8081"])
                .await
                .unwrap();
        });

        // TODO: connect stored peers.

        *runtime_lock = Some(runtime);

        Ok(())
    }

    pub fn stop(&self) -> anyhow::Result<()> {
        let mut runtime_lock = self.runtime.write().unwrap();
        if let Some(rt) = runtime_lock.take() {
            rt.shutdown_background();
            Ok(())
        } else {
            return Err(anyhow!("Runtime is not running."));
        }
    }

    async fn run_manager(
        manager: Arc<DlcDevKitDlcManager<S, O>>,
        receiver: Arc<Receiver<DlcManagerMessage>>,
    ) {
        while let Ok(msg) = receiver.recv() {
            match msg {
                DlcManagerMessage::OfferDlc {
                    contract_input,
                    counter_party,
                    oracle_announcements,
                    responder,
                } => {
                    let offer = manager
                        .send_offer_with_announcements(
                            &contract_input,
                            counter_party,
                            vec![oracle_announcements],
                        )
                        .expect("can't create offerdlc");
                    responder.send(offer).expect("send offer error")
                }
                DlcManagerMessage::AcceptDlc {
                    contract,
                    responder,
                } => {
                    let accept_dlc = manager.accept_contract_offer(&contract);

                    responder.send(accept_dlc).expect("can't send")
                }
                DlcManagerMessage::PeriodicCheck => {
                    manager.periodic_check(false).await.unwrap();
                }
            }
        }
    }

    pub fn connect_if_necessary(&self) -> anyhow::Result<()> {
        let _known_peers = self.storage.list_peers()?;

        // check from already connected

        Ok(())
    }

    pub fn network(&self) -> Network {
        self.network
    }

    pub fn send_dlc_offer(
        &self,
        contract_input: &ContractInput,
        counter_party: PublicKey,
        oracle_announcements: Vec<OracleAnnouncement>,
    ) -> anyhow::Result<OfferDlc> {
        let (responder, receiver) = unbounded();
        self.sender
            .send(DlcManagerMessage::OfferDlc {
                contract_input: contract_input.to_owned(),
                counter_party,
                oracle_announcements,
                responder,
            })
            .expect("sending offer message");
        let offer = receiver.recv().expect("no offer dlc");

        let contract_id = hex::encode(&offer.temporary_contract_id);
        self.transport
            .send_message(counter_party, Message::Offer(offer.clone()));
        tracing::info!(
            counterparty = counter_party.to_string(),
            contract_id,
            "Sent DLC offer to counterparty."
        );

        Ok(offer)
    }

    pub fn accept_dlc_offer(
        &self,
        contract: [u8; 32],
    ) -> anyhow::Result<(String, String, AcceptDlc)> {
        let (responder, receiver) = unbounded();
        self.sender.send(DlcManagerMessage::AcceptDlc {
            contract,
            responder,
        })?;

        let (contract_id, public_key, accept_dlc) = receiver.recv()?.map_err(|e| {
            tracing::error!(error=?e, "Could not accept offer.");
            anyhow!("Could not accept dlc offer.")
        })?;

        self.transport
            .send_message(public_key, Message::Accept(accept_dlc.clone()));

        let contract_id = hex::encode(&contract_id);
        let counter_party = public_key.to_string();
        tracing::info!(counter_party, contract_id, "Accepted DLC contract.");

        Ok((contract_id, counter_party, accept_dlc))
    }
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, SystemTime, UNIX_EPOCH};

    use crate::test_util::{generate_blocks, test_ddk, TestSuite};
    use dlc_manager::{
        contract::{contract_input::ContractInput, Contract},
        Storage,
    };
    use dlc_messages::{oracle_msgs::OracleAnnouncement, Message};
    use rstest::rstest;
    use tokio::time::sleep;

    #[rstest]
    #[test_log::test(tokio::test)]
    async fn contract_execution(
        #[future] test_ddk: (TestSuite, TestSuite, OracleAnnouncement, ContractInput),
    ) {
        let (alice, bob, announcement, contract_input) = test_ddk.await;
        let alice_makes_offer = alice.ddk.manager.send_offer_with_announcements(
            &contract_input,
            bob.ddk.transport.node_id,
            vec![vec![announcement.clone()]],
        );

        let alice_makes_offer = alice_makes_offer.expect("alice did not create an offer");

        let contract_id = alice_makes_offer.temporary_contract_id.clone();
        let alice_pubkey = alice.ddk.transport.node_id;
        let bob_pubkey = bob.ddk.transport.node_id;

        let bob_receives_offer = bob
            .ddk
            .manager
            .on_dlc_message(&Message::Offer(alice_makes_offer), alice_pubkey);

        let bob_receive_offer = bob_receives_offer.expect("bob did not receive the offer");
        assert!(bob_receive_offer.is_none());

        let bob_accept_offer = bob
            .ddk
            .manager
            .accept_contract_offer(&contract_id)
            .expect("bob could not accept offer");

        let (contract_id, _counter_party, bob_accept_dlc) = bob_accept_offer;

        let alice_receive_accept = alice
            .ddk
            .manager
            .on_dlc_message(&Message::Accept(bob_accept_dlc), bob_pubkey)
            .expect("alice did not receive accept");

        assert!(alice_receive_accept.is_some());

        let alice_sign_message = alice_receive_accept.unwrap();
        bob.ddk
            .manager
            .on_dlc_message(&alice_sign_message, alice_pubkey)
            .expect("bob did not receive sign message");

        generate_blocks(10);

        alice
            .ddk
            .manager
            .periodic_check(false)
            .await
            .expect("alice check failed");

        bob.ddk
            .manager
            .periodic_check(false)
            .await
            .expect("bob check failed");

        let contract = alice.ddk.storage.get_contract(&contract_id);
        assert!(matches!(contract.unwrap().unwrap(), Contract::Confirmed(_)));

        // Used to check that timelock is reached.
        let locktime = match alice.ddk.storage.get_contract(&contract_id).unwrap() {
            Some(contract) => match contract {
                Contract::Confirmed(signed_contract) => {
                    signed_contract.accepted_contract.dlc_transactions.cets[0]
                        .lock_time
                        .to_consensus_u32()
                }
                _ => unreachable!("No locktime."),
            },
            None => unreachable!("No locktime"),
        };

        let mut time = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs() as u32;

        let attestation = alice
            .ddk
            .oracle
            .sign_event(announcement.clone(), "rust".to_string())
            .await;

        while time < announcement.oracle_event.event_maturity_epoch || time < locktime {
            tracing::warn!("Waiting for time to expire for oracle event and locktime.");
            let checked_time = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_secs() as u32;

            time = checked_time;
            sleep(Duration::from_secs(5)).await
        }

        assert!(attestation.is_ok());

        bob.ddk.manager.periodic_check(false).await.unwrap();

        let contract = alice.ddk.storage.get_contract(&contract_id);
        assert!(matches!(contract.unwrap().unwrap(), Contract::PreClosed(_)));

        generate_blocks(10);

        let contract = alice.ddk.storage.get_contract(&contract_id);
        assert!(matches!(contract.unwrap().unwrap(), Contract::Closed(_)));
    }
}

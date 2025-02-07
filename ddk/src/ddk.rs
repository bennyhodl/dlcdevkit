use crate::chain::EsploraClient;
use crate::wallet::DlcDevKitWallet;
#[cfg(feature = "marketplace")]
use crate::{nostr::marketplace::*, DEFAULT_NOSTR_RELAY};
use crate::{Oracle, Storage, Transport};
use anyhow::anyhow;
use bitcoin::secp256k1::PublicKey;
use bitcoin::{Amount, Network};
use crossbeam::channel::{unbounded, Receiver, Sender};
use ddk_manager::contract::Contract;
use ddk_manager::error::Error;
use ddk_manager::{
    contract::contract_input::ContractInput, CachedContractSignerProvider, ContractId,
    SimpleSigner, SystemTimeProvider,
};
use dlc_messages::oracle_msgs::OracleAnnouncement;
use dlc_messages::{AcceptDlc, Message, OfferDlc};
use std::sync::{Arc, RwLock};
use std::time::Duration;
use tokio::runtime::Runtime;
use tokio::sync::watch;

/// DlcDevKit type alias for the [ddk_manager::manager::Manager]
pub type DlcDevKitDlcManager<S, O> = ddk_manager::manager::Manager<
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
    pub stop_signal: watch::Receiver<bool>,
    pub stop_signal_sender: watch::Sender<bool>,
}

impl<T, S, O> DlcDevKit<T, S, O>
where
    T: Transport,
    S: Storage,
    O: Oracle,
{
    pub fn start(&self) -> anyhow::Result<()> {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .unwrap();
        self.start_with_runtime(runtime)
    }

    pub fn start_with_runtime(&self, runtime: Runtime) -> anyhow::Result<()> {
        let mut runtime_lock = self.runtime.write().unwrap();

        if runtime_lock.is_some() {
            return Err(anyhow!("DDK is still running."));
        }

        let manager_clone = self.manager.clone();
        let receiver_clone = self.receiver.clone();
        runtime.spawn(async move { Self::run_manager(manager_clone, receiver_clone).await });

        let transport_clone = self.transport.clone();
        let manager_clone = self.manager.clone();
        let stop_signal = self.stop_signal.clone();
        runtime.spawn(async move {
            if let Err(e) = transport_clone.start(stop_signal, manager_clone).await {
                tracing::error!(error = e.to_string(), "Error in transport listeners.");
            }
        });

        let wallet_clone = self.wallet.clone();
        runtime.spawn(async move {
            let mut timer = tokio::time::interval(Duration::from_secs(60));
            loop {
                timer.tick().await;
                if let Err(e) = wallet_clone.sync().await {
                    tracing::warn!(error=?e, "Did not sync wallet.");
                };
            }
        });

        let processor = self.sender.clone();
        runtime.spawn(async move {
            let mut timer = tokio::time::interval(Duration::from_secs(30));
            loop {
                timer.tick().await;
                processor
                    .send(DlcManagerMessage::PeriodicCheck)
                    .expect("couldn't send periodic check");
            }
        });

        #[cfg(feature = "marketplace")]
        {
            let storage_clone = self.storage.clone();
            runtime.spawn(async move {
                tracing::info!("Starting marketplace listener.");
                marketplace_listener(&storage_clone, vec![DEFAULT_NOSTR_RELAY])
                    .await
                    .unwrap();
            });
        }

        // TODO: connect stored peers.

        *runtime_lock = Some(runtime);
        Ok(())
    }

    pub fn stop(&self) -> anyhow::Result<()> {
        tracing::warn!("Shutting down DDK runtime and listeners.");
        self.stop_signal_sender.send(true)?;
        let mut runtime_lock = self.runtime.write().unwrap();
        if let Some(rt) = runtime_lock.take() {
            rt.shutdown_background();
            Ok(())
        } else {
            Err(anyhow!("Runtime is not running."))
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
                    oracle_announcements: _,
                    responder,
                } => {
                    let offer = manager
                        .send_offer(&contract_input, counter_party)
                        .await
                        .expect("can't create offerdlc");

                    responder.send(offer).expect("send offer error")
                }
                DlcManagerMessage::AcceptDlc {
                    contract,
                    responder,
                } => {
                    let accept_dlc = manager.accept_contract_offer(&contract).await;

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

    pub async fn send_dlc_offer(
        &self,
        contract_input: &ContractInput,
        counter_party: PublicKey,
        oracle_announcements: Vec<OracleAnnouncement>,
    ) -> anyhow::Result<OfferDlc> {
        let (responder, receiver) = unbounded();
        self.sender.send(DlcManagerMessage::OfferDlc {
            contract_input: contract_input.to_owned(),
            counter_party,
            oracle_announcements,
            responder,
        })?;
        let offer = receiver.recv()?;

        let contract_id = hex::encode(offer.temporary_contract_id);
        self.transport
            .send_message(counter_party, Message::Offer(offer.clone()))
            .await;
        tracing::info!(
            counterparty = counter_party.to_string(),
            contract_id,
            "Sent DLC offer to counterparty."
        );

        Ok(offer)
    }

    pub async fn accept_dlc_offer(
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
            .send_message(public_key, Message::Accept(accept_dlc.clone()))
            .await;

        let contract_id = hex::encode(contract_id);
        let counter_party = public_key.to_string();
        tracing::info!(
            counter_party,
            contract_id,
            "Accepted and sent accept DLC contract."
        );

        Ok((contract_id, counter_party, accept_dlc))
    }

    pub async fn balance(&self) -> anyhow::Result<crate::Balance> {
        let wallet_balance = self.wallet.get_balance()?;
        let contracts = self.storage.get_contracts().await?;

        let contract = &contracts
            .iter()
            .map(|contract| match contract {
                Contract::Confirmed(c) => {
                    let accept_party_collateral = c.accepted_contract.accept_params.collateral;
                    if c.accepted_contract.offered_contract.is_offer_party {
                        Amount::from_sat(
                            c.accepted_contract.offered_contract.total_collateral
                                - accept_party_collateral,
                        )
                    } else {
                        Amount::from_sat(c.accepted_contract.accept_params.collateral)
                    }
                }
                _ => Amount::ZERO,
            })
            .sum::<Amount>();

        let contract_pnl = &contracts
            .iter()
            .map(|contract| match contract {
                Contract::Closed(_) => 0_i64,
                Contract::PreClosed(p) => p
                    .signed_contract
                    .accepted_contract
                    .compute_pnl(&p.signed_cet),
                _ => 0_i64,
            })
            .sum::<i64>();

        Ok(crate::Balance {
            confirmed: wallet_balance.confirmed,
            change_unconfirmed: wallet_balance.immature + wallet_balance.trusted_pending,
            foreign_unconfirmed: wallet_balance.untrusted_pending,
            contract: contract.to_owned(),
            contract_pnl: contract_pnl.to_owned(),
        })
    }
}

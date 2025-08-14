use crate::{ddk::DlcDevKitDlcManager, error::TransportError, Oracle, Storage, Transport};
use bitcoin::{
    key::{self, Keypair},
    secp256k1::{All, PublicKey, Secp256k1},
};
use std::{collections::HashMap, sync::Arc, time::Duration};
// use crossbeam::channel::{unbounded, Receiver, Sender};
use ddk_messages::Message;
use tokio::sync::mpsc::{channel, Receiver, Sender};
use tokio::sync::watch;
use tokio::sync::Mutex;

type CounterPartyTransport = Arc<Mutex<HashMap<PublicKey, Sender<(Message, PublicKey)>>>>;
pub struct MemoryTransport {
    pub receiver: Arc<Mutex<Receiver<(Message, PublicKey)>>>,
    pub sender: Sender<(Message, PublicKey)>,
    pub counterparty_transport: CounterPartyTransport,
    pub keypair: Keypair,
}

impl MemoryTransport {
    pub fn new(secp: &Secp256k1<All>) -> Self {
        let (sender, receiver) = channel(100);
        let keypair = Keypair::new(secp, &mut key::rand::thread_rng());
        Self {
            receiver: Arc::new(Mutex::new(receiver)),
            sender,
            counterparty_transport: Arc::new(Mutex::new(HashMap::new())),
            keypair,
        }
    }

    pub async fn add_counterparty(
        &self,
        counterparty: PublicKey,
        sender: Sender<(Message, PublicKey)>,
    ) {
        let mut guard = self.counterparty_transport.lock().await;
        guard.insert(counterparty, sender);
        drop(guard)
    }
}

#[async_trait::async_trait]
impl Transport for MemoryTransport {
    fn name(&self) -> String {
        "memory transport".to_string()
    }

    fn public_key(&self) -> PublicKey {
        self.keypair.public_key()
    }

    async fn send_message(&self, counterparty: PublicKey, message: Message) {
        let counterparties = self.counterparty_transport.lock().await;
        let connected_counterparty = counterparties.get(&counterparty);
        if let Some(counterparty) = connected_counterparty {
            counterparty
                .send((message, self.keypair.public_key()))
                .await
                .expect("could not send message to counterparty")
        } else {
            tracing::error!("No counterparty connected.")
        }
    }

    async fn start<S: Storage, O: Oracle>(
        &self,
        mut stop_receiver: watch::Receiver<bool>,
        manager: Arc<DlcDevKitDlcManager<S, O>>,
    ) -> Result<(), TransportError> {
        let mut timer = tokio::time::interval(Duration::from_secs(1));
        let receiver = self.receiver.clone();
        loop {
            tokio::select! {
                _ = stop_receiver.changed() => {
                    if *stop_receiver.borrow() {
                        break;
                    }
                },
                _ = timer.tick() => {
                    if let Some(msg) = receiver.lock().await.recv().await {
                        match manager.on_dlc_message(&msg.0, msg.1).await {
                            Ok(s) => {
                                if let Some(reply) = s {
                                    self.send_message(msg.1, reply).await;
                                } else {
                                    tracing::info!("Handled on_dlc_message.");
                                }
                            }
                            Err(e) => tracing::error!(
                                error = e.to_string(),
                                "In memory transport error on dlc message."
                            ),
                        }
                    }
                }
            }
        }
        Ok(())
    }

    async fn connect_outbound(&self, _pubkey: PublicKey, _host: &str) {
        unreachable!("no need to connect to counterparty")
    }
}

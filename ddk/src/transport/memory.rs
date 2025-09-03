use crate::logger::{log_error, log_info, WriteLog};
use crate::{
    ddk::DlcDevKitDlcManager, error::TransportError, logger::Logger, Oracle, Storage, Transport,
};
use bitcoin::{
    key::{self, Keypair},
    secp256k1::{All, PublicKey, Secp256k1},
};
use ddk_messages::Message;
use std::{collections::HashMap, sync::Arc, time::Duration};
use tokio::sync::mpsc::{channel, Receiver, Sender};
use tokio::sync::watch;
use tokio::sync::Mutex;

type CounterPartyTransport = Arc<Mutex<HashMap<PublicKey, Sender<(Message, PublicKey)>>>>;
pub struct MemoryTransport {
    pub receiver: Arc<Mutex<Receiver<(Message, PublicKey)>>>,
    pub sender: Sender<(Message, PublicKey)>,
    pub counterparty_transport: CounterPartyTransport,
    pub keypair: Keypair,
    pub logger: Arc<Logger>,
}

impl MemoryTransport {
    pub fn new(secp: &Secp256k1<All>, logger: Arc<Logger>) -> Self {
        let (sender, receiver) = channel(100);
        let keypair = Keypair::new(secp, &mut key::rand::thread_rng());
        Self {
            receiver: Arc::new(Mutex::new(receiver)),
            sender,
            counterparty_transport: Arc::new(Mutex::new(HashMap::new())),
            keypair,
            logger,
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
            log_error!(self.logger, "No counterparty connected.");
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
                                    log_info!(self.logger, "Handled on_dlc_message.");
                                }
                            }
                            Err(e) => log_error!(
                                self.logger,
                                "In memory transport error on dlc message. error={}",
                                e.to_string()
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
